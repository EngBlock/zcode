//! Files, groups and target membership in `project.pbxproj`, and the
//! structural edits Xcode makes when the project navigator adds, renames,
//! moves or deletes files: `PBXFileReference`s in `PBXGroup`s, and
//! `PBXBuildFile`s in the targets' build phases.
//!
//! Folders inside a `PBXFileSystemSynchronizedRootGroup` (Xcode 16+) are
//! left alone: Xcode derives their contents from disk.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, anyhow};

use super::Node;
use super::edit::{Batch, Field, NewObject, Position};
use crate::paths::relative_path;

pub const SYNCHRONIZED_GROUP: &str = "PBXFileSystemSynchronizedRootGroup";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseKind {
    Sources,
    Resources,
    Headers,
    Frameworks,
}

impl PhaseKind {
    pub fn isa(self) -> &'static str {
        match self {
            Self::Sources => "PBXSourcesBuildPhase",
            Self::Resources => "PBXResourcesBuildPhase",
            Self::Headers => "PBXHeadersBuildPhase",
            Self::Frameworks => "PBXFrameworksBuildPhase",
        }
    }

    /// The name Xcode gives the phase in `ID /* X in Sources */` comments.
    pub fn name(self) -> &'static str {
        match self {
            Self::Sources => "Sources",
            Self::Resources => "Resources",
            Self::Headers => "Headers",
            Self::Frameworks => "Frameworks",
        }
    }

    /// The phase a file of this type is built in, as Xcode picks it.
    pub fn for_file(file_type: &str, file_name: &str) -> Option<Self> {
        if file_name == "Info.plist" {
            return None;
        }
        match file_type {
            "sourcecode.swift" | "sourcecode.c.objc" | "sourcecode.cpp.objcpp" | "sourcecode.c.c" | "sourcecode.cpp.cpp"
            | "sourcecode.metal" | "sourcecode.asm" | "sourcecode.mig" | "sourcecode.lex" | "sourcecode.yacc"
            | "sourcecode.iig" | "file.intentdefinition" | "wrapper.xcdatamodel" | "wrapper.xcdatamodeld"
            | "wrapper.xcmappingmodel" | "folder.documentationcatalog" | "file.mlmodel" | "folder.mlpackage" => {
                Some(Self::Sources)
            }
            "sourcecode.c.h" | "sourcecode.cpp.h" => Some(Self::Headers),
            "wrapper.framework" | "wrapper.xcframework" | "archive.ar" | "compiled.mach-o.dylib"
            | "sourcecode.text-based-dylib-definition" => Some(Self::Frameworks),
            "text.xcconfig" | "text.plist.entitlements" | "sourcecode.module-map" | "wrapper.application"
            | "wrapper.cfbundle" => None,
            _ => Some(Self::Resources),
        }
    }
}

/// Xcode's `lastKnownFileType` for a path.
pub fn file_type_for(path: &Path, is_dir: bool) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "swift" => "sourcecode.swift",
        "m" => "sourcecode.c.objc",
        "mm" => "sourcecode.cpp.objcpp",
        "c" => "sourcecode.c.c",
        "cc" | "cpp" | "cxx" => "sourcecode.cpp.cpp",
        "h" => "sourcecode.c.h",
        "hh" | "hpp" | "hxx" => "sourcecode.cpp.h",
        "metal" => "sourcecode.metal",
        "s" => "sourcecode.asm",
        "modulemap" => "sourcecode.module-map",
        "js" => "sourcecode.javascript",
        "plist" => "text.plist.xml",
        "strings" => "text.plist.strings",
        "stringsdict" => "text.plist.stringsdict",
        "xcstrings" => "text.json.xcstrings",
        "entitlements" => "text.plist.entitlements",
        "xcconfig" => "text.xcconfig",
        "xcprivacy" => "text.xml",
        "json" => "text.json",
        "xml" => "text.xml",
        "html" | "htm" => "text.html",
        "yml" | "yaml" => "text.yaml",
        "md" | "markdown" => "net.daringfireball.markdown",
        "txt" => "text",
        "sh" => "text.script.sh",
        "py" => "text.script.python",
        "rb" => "text.script.ruby",
        "storyboard" => "file.storyboard",
        "xib" => "file.xib",
        "xcassets" => "folder.assetcatalog",
        "png" => "image.png",
        "jpg" | "jpeg" => "image.jpeg",
        "gif" => "image.gif",
        "pdf" => "image.pdf",
        "svg" => "image.svg",
        "heic" => "image.heic",
        "mp3" => "audio.mp3",
        "wav" => "audio.wav",
        "mov" => "video.quicktime",
        "intentdefinition" => "file.intentdefinition",
        "xcdatamodel" => "wrapper.xcdatamodel",
        "xcdatamodeld" => "wrapper.xcdatamodeld",
        "xcmappingmodel" => "wrapper.xcmappingmodel",
        "mlmodel" => "file.mlmodel",
        "mlpackage" => "folder.mlpackage",
        "docc" => "folder.documentationcatalog",
        "framework" => "wrapper.framework",
        "xcframework" => "wrapper.xcframework",
        "a" => "archive.ar",
        "dylib" => "compiled.mach-o.dylib",
        "tbd" => "sourcecode.text-based-dylib-definition",
        "bundle" => "wrapper.plug-in",
        "app" => "wrapper.application",
        "scnassets" => "wrapper.scnassets",
        _ if is_dir => "folder",
        _ => "file",
    }
}

/// Folders Xcode treats as single files (bundles, catalogs) rather than
/// groups to recurse into. `.lproj` folders are added as folder references so
/// their localized resources keep their folder in the built bundle.
fn is_opaque_folder(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).unwrap_or(""),
        "xcassets" | "framework" | "xcframework" | "bundle" | "app" | "appex" | "docc" | "mlpackage" | "scnassets"
            | "xcdatamodel" | "xcdatamodeld" | "xcmappingmodel" | "lproj" | "playground" | "rcproject" | "plugin"
    )
}

/// Removes `.` and `..` components without touching the file system.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The canonical form of a path that may no longer (or not yet) exist: its
/// longest existing ancestor canonicalized, plus the rest.
pub fn canonical(path: &Path) -> PathBuf {
    let path = normalize(path);
    let mut existing = path.as_path();
    loop {
        if let Ok(real) = existing.canonicalize() {
            let rest = path.strip_prefix(existing).unwrap_or(Path::new(""));
            return if rest.as_os_str().is_empty() { real } else { real.join(rest) };
        }
        match existing.parent() {
            Some(parent) => existing = parent,
            None => return path,
        }
    }
}

// ---------------------------------------------------------------------------
// The index

/// A group or file reference in the project's group tree.
#[derive(Clone, Debug)]
pub struct Item {
    pub id: String,
    pub isa: String,
    pub name: Option<String>,
    pub path: Option<String>,
    pub source_tree: String,
    pub file_type: Option<String>,
    pub parent: Option<String>,
    pub children: Vec<String>,
    /// The folder `path` is relative to, when it is a real folder.
    pub base: Option<PathBuf>,
    /// Where the item is on disk, when its source tree is a real folder.
    pub resolved: Option<PathBuf>,
    pub depth: usize,
}

impl Item {
    pub fn is_group(&self) -> bool {
        matches!(self.isa.as_str(), "PBXGroup" | "PBXVariantGroup" | "XCVersionGroup" | SYNCHRONIZED_GROUP)
    }

    /// The name Xcode shows, and writes in `ID /* name */` comments.
    pub fn display_name(&self) -> Option<&str> {
        self.name.as_deref().or(self.path.as_deref())
    }

    pub fn location_label(&self) -> &str {
        match self.source_tree.as_str() {
            "<group>" => "Relative to Group",
            "SOURCE_ROOT" => "Relative to Project",
            "<absolute>" => "Absolute Path",
            "BUILT_PRODUCTS_DIR" => "Relative to Build Products",
            "DEVELOPER_DIR" => "Relative to Developer Directory",
            "SDKROOT" => "Relative to SDK",
            other => other,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Phase {
    pub id: String,
    pub isa: String,
    pub name: Option<String>,
    /// Build file IDs.
    pub files: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Target {
    pub id: String,
    pub name: String,
    pub isa: String,
    pub product_type: Option<String>,
    pub phases: Vec<Phase>,
    pub synchronized_groups: Vec<String>,
}

impl Target {
    pub fn phase(&self, kind: PhaseKind) -> Option<&Phase> {
        self.phases.iter().find(|p| p.isa == kind.isa())
    }

    /// Products that are bundles, so they can have resources.
    fn is_bundle(&self) -> bool {
        !matches!(
            self.product_type.as_deref().unwrap_or(""),
            "com.apple.product-type.tool" | "com.apple.product-type.library.static" | "com.apple.product-type.library.dynamic" | ""
        )
    }

    /// Whether files built in `kind` can be members of this target.
    pub fn accepts(&self, kind: PhaseKind) -> bool {
        self.isa == "PBXNativeTarget"
            && match kind {
                PhaseKind::Sources => true,
                PhaseKind::Resources => self.phase(kind).is_some() || self.is_bundle(),
                PhaseKind::Headers | PhaseKind::Frameworks => self.phase(kind).is_some(),
            }
    }
}

/// The files and groups of one `.xcodeproj`.
#[derive(Clone, Debug)]
pub struct ProjectFiles {
    pub project_dir: PathBuf,
    pub main_group: String,
    pub products_group: Option<String>,
    pub items: HashMap<String, Item>,
    /// Item IDs in navigator order (depth first).
    pub order: Vec<String>,
    pub targets: Vec<Target>,
    /// Build file → file reference.
    pub build_files: HashMap<String, String>,
    /// Item IDs by folder or file, in navigator order.
    by_path: HashMap<PathBuf, Vec<String>>,
    synchronized_roots: Vec<String>,
}

impl ProjectFiles {
    pub fn parse(xcodeproj: &Path, text: &str) -> Result<Self> {
        Self::from_root(xcodeproj, &super::parse(text)?)
    }

    pub fn from_root(xcodeproj: &Path, root: &Node) -> Result<Self> {
        let objects_node = root.get("objects").ok_or_else(|| anyhow!("no objects"))?;
        let objects: HashMap<&str, &Node> = objects_node.entries().iter().map(|e| (e.key.as_str(), &e.value)).collect();
        let project_id = root.get_str("rootObject").ok_or_else(|| anyhow!("no rootObject"))?;
        let project = *objects.get(project_id).ok_or_else(|| anyhow!("missing project object"))?;
        let dir = xcodeproj.parent().unwrap_or(Path::new("/")).join(project.get_str("projectDirPath").unwrap_or(""));
        let project_dir = canonical(&dir);
        let main_group = project.get_str("mainGroup").ok_or_else(|| anyhow!("no mainGroup"))?.to_string();

        let mut items: HashMap<String, Item> = HashMap::new();
        let mut order = vec![];
        let mut stack: Vec<(String, Option<String>, usize)> = vec![(main_group.clone(), None, 0)];
        while let Some((id, parent, depth)) = stack.pop() {
            let Some(node) = objects.get(id.as_str()) else { continue };
            if items.contains_key(&id) {
                continue;
            }
            let source_tree = node.get_str("sourceTree").unwrap_or("<group>").to_string();
            let base = match source_tree.as_str() {
                "<group>" => match &parent {
                    Some(parent) => items.get(parent).and_then(|p| p.resolved.clone()),
                    None => Some(project_dir.clone()),
                },
                "SOURCE_ROOT" => Some(project_dir.clone()),
                "<absolute>" => Some(PathBuf::from("/")),
                _ => None,
            };
            let path = node.get_str("path").map(str::to_string);
            let resolved = base.as_ref().map(|base| match &path {
                Some(path) => normalize(&base.join(path)),
                None => base.clone(),
            });
            let children: Vec<String> =
                node.get("children").map(Node::items).unwrap_or_default().iter().filter_map(Node::as_str).map(str::to_string).collect();
            for child in children.iter().rev() {
                stack.push((child.clone(), Some(id.clone()), depth + 1));
            }
            order.push(id.clone());
            items.insert(
                id.clone(),
                Item {
                    id,
                    isa: node.get_str("isa").unwrap_or("").to_string(),
                    name: node.get_str("name").map(str::to_string),
                    path,
                    source_tree,
                    file_type: node.get_str("lastKnownFileType").or(node.get_str("explicitFileType")).map(str::to_string),
                    parent,
                    children,
                    base,
                    resolved,
                    depth,
                },
            );
        }

        let ids = |node: Option<&Node>| -> Vec<String> {
            node.map(Node::items).unwrap_or_default().iter().filter_map(Node::as_str).map(str::to_string).collect()
        };
        let targets = ids(project.get("targets"))
            .into_iter()
            .filter_map(|id| {
                let target = objects.get(id.as_str())?;
                let phases = ids(target.get("buildPhases"))
                    .into_iter()
                    .filter_map(|phase_id| {
                        let phase = objects.get(phase_id.as_str())?;
                        Some(Phase {
                            isa: phase.get_str("isa").unwrap_or("").to_string(),
                            name: phase.get_str("name").map(str::to_string),
                            files: ids(phase.get("files")),
                            id: phase_id,
                        })
                    })
                    .collect();
                Some(Target {
                    name: target.get_str("name").unwrap_or(&id).to_string(),
                    isa: target.get_str("isa").unwrap_or("").to_string(),
                    product_type: target.get_str("productType").map(str::to_string),
                    phases,
                    synchronized_groups: ids(target.get("fileSystemSynchronizedGroups")),
                    id,
                })
            })
            .collect();
        let build_files = objects_node
            .entries()
            .iter()
            .filter(|e| e.value.get_str("isa") == Some("PBXBuildFile"))
            .filter_map(|e| Some((e.key.clone(), e.value.get_str("fileRef")?.to_string())))
            .collect();

        let mut by_path: HashMap<PathBuf, Vec<String>> = HashMap::new();
        for id in &order {
            if let Some(resolved) = &items[id].resolved {
                by_path.entry(resolved.clone()).or_default().push(id.clone());
            }
        }
        let synchronized_roots = order.iter().filter(|id| items[*id].isa == SYNCHRONIZED_GROUP).cloned().collect();
        Ok(Self {
            project_dir,
            main_group,
            products_group: project.get_str("productRefGroup").map(str::to_string),
            items,
            order,
            targets,
            build_files,
            by_path,
            synchronized_roots,
        })
    }

    fn in_order(&self) -> impl Iterator<Item = &Item> {
        self.order.iter().filter_map(|id| self.items.get(id))
    }

    pub fn target(&self, id: &str) -> Option<&Target> {
        self.targets.iter().find(|t| t.id == id)
    }

    /// The synchronized folder containing `path`, if any.
    pub fn synchronized_root(&self, path: &Path) -> Option<&Item> {
        self.synchronized_roots
            .iter()
            .filter_map(|id| self.items.get(id))
            .find(|i| i.resolved.as_deref().is_some_and(|r| path.starts_with(r)))
    }

    fn at_path(&self, path: &Path) -> impl Iterator<Item = &Item> {
        self.by_path.get(path).into_iter().flatten().filter_map(|id| self.items.get(id))
    }

    pub fn file_reference(&self, path: &Path) -> Option<&Item> {
        self.at_path(path).find(|i| i.isa == "PBXFileReference")
    }

    /// The group that stands for folder `dir`: one with its own `path`
    /// rather than a virtual group sharing its parent's folder, and the
    /// outermost of several.
    pub fn group_for_dir(&self, dir: &Path) -> Option<&Item> {
        self.at_path(dir)
            .filter(|i| i.isa == "PBXGroup")
            .min_by_key(|i| (i.path.is_none() && i.id != self.main_group, i.depth))
    }

    fn main_dir(&self) -> Option<&Path> {
        self.items.get(&self.main_group)?.resolved.as_deref()
    }

    /// Whether `path` belongs in this project's group tree: under its folder
    /// and not in a synchronized folder.
    pub fn contains(&self, path: &Path) -> bool {
        self.synchronized_root(path).is_none() && self.main_dir().is_some_and(|m| path.starts_with(m))
    }

    /// Whether `path` is in this project's folder at all, synchronized or not.
    pub fn covers(&self, path: &Path) -> bool {
        self.main_dir().is_some_and(|m| path.starts_with(m))
    }

    /// Target IDs whose phases build file reference `id`, with the build file.
    pub fn memberships(&self, id: &str) -> Vec<(&Target, &Phase, &str)> {
        let mut out = vec![];
        for target in &self.targets {
            for phase in &target.phases {
                for build_file in &phase.files {
                    if self.build_files.get(build_file).map(String::as_str) == Some(id) {
                        out.push((target, phase, build_file.as_str()));
                    }
                }
            }
        }
        out
    }

    /// The targets a new file at `path` should join, as Xcode preselects
    /// them: the targets that build the other files of the same kind in its
    /// group (or the nearest group above with any), otherwise the first app.
    pub fn suggested_targets(&self, path: &Path) -> Vec<String> {
        let file_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let Some(kind) = PhaseKind::for_file(file_type_for(path, path.is_dir()), &file_name) else { return vec![] };
        let mut dir = path.parent();
        while let Some(d) = dir {
            if let Some(group) = self.group_for_dir(d) {
                if group.id == self.main_group {
                    break;
                }
                let mut found: Vec<String> = vec![];
                let mut stack = vec![group.id.as_str()];
                while let Some(id) = stack.pop() {
                    let Some(item) = self.items.get(id) else { continue };
                    stack.extend(item.children.iter().map(String::as_str));
                    for (target, phase, _) in self.memberships(id) {
                        if phase.isa == kind.isa() && !found.contains(&target.id) {
                            found.push(target.id.clone());
                        }
                    }
                }
                if !found.is_empty() {
                    // In project order.
                    return self.targets.iter().filter(|t| found.contains(&t.id)).map(|t| t.id.clone()).collect();
                }
            }
            if self.main_dir().is_some_and(|m| d == m) {
                break;
            }
            dir = d.parent();
        }
        let app = self.targets.iter().find(|t| t.product_type.as_deref() == Some("com.apple.product-type.application"));
        app.or_else(|| self.targets.iter().find(|t| t.accepts(kind)))
            .filter(|t| t.accepts(kind))
            .map(|t| vec![t.id.clone()])
            .unwrap_or_default()
    }

    /// What the File inspector shows for `path`.
    pub fn file_info(&self, path: &Path) -> FileInfo {
        let path = canonical(path);
        let is_dir = path.is_dir();
        let file_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if let Some(root) = self.synchronized_root(&path) {
            let targets = self
                .targets
                .iter()
                .filter(|t| t.synchronized_groups.contains(&root.id))
                .map(|t| t.name.clone())
                .collect();
            return FileInfo {
                file_type: file_type_for(&path, is_dir).to_string(),
                location: "Relative to Group".into(),
                tracking: Tracking::Synchronized {
                    folder: root.display_name().unwrap_or("").to_string(),
                    targets,
                },
            };
        }
        let reference = self
            .file_reference(&path)
            .or_else(|| self.at_path(&path).find(|i| i.isa == "PBXGroup" && i.path.is_some()));
        let file_type = reference
            .and_then(|r| r.file_type.clone())
            .unwrap_or_else(|| if reference.is_some_and(Item::is_group) { "Group".into() } else { file_type_for(&path, is_dir).to_string() });
        let in_tree = self.main_dir().is_some_and(|m| path.starts_with(m));
        if reference.is_some_and(Item::is_group) || (is_dir && !is_opaque_folder(&path)) {
            return FileInfo {
                file_type,
                location: reference.map(|r| r.location_label().to_string()).unwrap_or_default(),
                tracking: if reference.is_some() { Tracking::Group } else { Tracking::NotInProject { can_add: false } },
            };
        }
        let kind = PhaseKind::for_file(&file_type, &file_name);
        let Some(reference) = reference else {
            return FileInfo {
                file_type,
                location: String::new(),
                tracking: if in_tree {
                    Tracking::Referenced { referenced: false, targets: self.target_rows(None, kind) }
                } else {
                    Tracking::NotInProject { can_add: false }
                },
            };
        };
        FileInfo {
            file_type,
            location: reference.location_label().to_string(),
            tracking: Tracking::Referenced { referenced: true, targets: self.target_rows(Some(&reference.id), kind) },
        }
    }

    fn target_rows(&self, reference: Option<&str>, kind: Option<PhaseKind>) -> Vec<TargetMembership> {
        let members = reference.map(|r| self.memberships(r)).unwrap_or_default();
        self.targets
            .iter()
            .filter(|t| t.isa == "PBXNativeTarget")
            .map(|t| TargetMembership {
                id: t.id.clone(),
                name: t.name.clone(),
                product_type: t.product_type.clone(),
                member: members.iter().any(|(m, _, _)| m.id == t.id),
                available: kind.is_some_and(|k| t.accepts(k)),
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TargetMembership {
    pub id: String,
    pub name: String,
    pub product_type: Option<String>,
    pub member: bool,
    /// Whether this kind of file can be built by the target.
    pub available: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tracking {
    /// In a synchronized folder: its targets build it.
    Synchronized { folder: String, targets: Vec<String> },
    /// A file the project can reference; `referenced` once it does.
    Referenced { referenced: bool, targets: Vec<TargetMembership> },
    /// A group (folder) in the project.
    Group,
    NotInProject { can_add: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileInfo {
    pub file_type: String,
    pub location: String,
    pub tracking: Tracking,
}

// ---------------------------------------------------------------------------
// Edits

#[derive(Clone, Debug, Default)]
pub struct AddOptions {
    /// Add folders as one folder reference instead of groups of files.
    pub folder_references: bool,
    /// The group for files outside the project's folder (the main group if
    /// unset), given by its folder.
    pub destination: Option<PathBuf>,
}

struct Editor<'a> {
    batch: Batch<'a>,
    files: ProjectFiles,
    /// Groups created in this batch, by folder.
    new_groups: Vec<(PathBuf, String)>,
    /// Build phases created in this batch: (target, kind) → phase.
    new_phases: HashMap<(String, &'static str), String>,
    changed: bool,
}

impl<'a> Editor<'a> {
    fn new(xcodeproj: &Path, text: &'a str) -> Result<Self> {
        let batch = Batch::new(text)?;
        let files = ProjectFiles::from_root(xcodeproj, &batch.root)?;
        Ok(Self { batch, files, new_groups: vec![], new_phases: HashMap::new(), changed: false })
    }

    fn finish(self) -> Result<Option<String>> {
        if !self.changed {
            return Ok(None);
        }
        self.batch.finish().map(Some)
    }

    fn group_dir(&self, group: &str) -> Option<PathBuf> {
        self.new_groups
            .iter()
            .find(|(_, id)| id == group)
            .map(|(dir, _)| dir.clone())
            .or_else(|| self.files.items.get(group)?.resolved.clone())
    }

    fn display_name(&self, id: &str) -> Option<String> {
        if let Some(object) = self.batch.new_objects.iter().find(|o| o.id == id) {
            return object.comment.clone();
        }
        self.files.items.get(id)?.display_name().map(str::to_string)
    }

    /// The group for folder `dir`, creating groups for it and any missing
    /// folders between it and the nearest existing group, as Xcode does when
    /// a file is created in a folder the project has no group for.
    fn ensure_group(&mut self, dir: &Path) -> Option<String> {
        if let Some((_, id)) = self.new_groups.iter().find(|(d, _)| d == dir) {
            return Some(id.clone());
        }
        if let Some(group) = self.files.group_for_dir(dir).filter(|g| !self.batch.is_removed(&g.id)) {
            return Some(group.id.clone());
        }
        let main_dir = self.files.main_dir()?.to_path_buf();
        if !dir.starts_with(&main_dir) || dir == main_dir {
            return None;
        }
        let parent = self.ensure_group(dir.parent()?)?;
        let name = dir.file_name()?.to_string_lossy().to_string();
        let id = self.batch.ids.next();
        self.batch.add_object(NewObject {
            id: id.clone(),
            isa: "PBXGroup",
            comment: Some(name.clone()),
            fields: vec![
                ("children", Field::List(vec![])),
                ("path", Field::Str(name.clone())),
                ("sourceTree", Field::Str("<group>".into())),
            ],
            one_line: false,
        });
        self.add_child(&parent, &id, &name);
        self.new_groups.push((dir.to_path_buf(), id.clone()));
        Some(id)
    }

    /// Adds `id` to a group's children: in sorted position when the group
    /// is already sorted by name, otherwise at the end.
    fn add_child(&mut self, group: &str, id: &str, name: &str) {
        let position = match self.files.items.get(group).filter(|_| !self.batch.is_new(group)) {
            Some(item) => {
                let names: Vec<(String, String)> = item
                    .children
                    .iter()
                    .filter_map(|c| Some((c.clone(), self.display_name(c)?.to_lowercase())))
                    .collect();
                let sorted = names.windows(2).all(|w| w[0].1 <= w[1].1);
                let lower = name.to_lowercase();
                match names.iter().find(|(_, n)| *n > lower) {
                    Some((next, _)) if sorted => Position::Before(next.clone()),
                    _ => Position::End,
                }
            }
            None => Position::End,
        };
        self.batch.add_to_list(group, "children", id, Some(name.to_string()), position);
        self.changed = true;
    }

    /// The phase of `kind` in `target`, created when the target lacks one.
    fn phase_for(&mut self, target: &Target, kind: PhaseKind) -> Option<(String, String)> {
        if let Some(phase) = target.phase(kind) {
            return Some((phase.id.clone(), phase.name.clone().unwrap_or_else(|| kind.name().to_string())));
        }
        if let Some(id) = self.new_phases.get(&(target.id.clone(), kind.isa())) {
            return Some((id.clone(), kind.name().to_string()));
        }
        if !target.accepts(kind) {
            return None;
        }
        let id = self.batch.ids.next();
        self.batch.add_object(NewObject {
            id: id.clone(),
            isa: kind.isa(),
            comment: Some(kind.name().into()),
            fields: vec![
                ("buildActionMask", Field::Str("2147483647".into())),
                ("files", Field::List(vec![])),
                ("runOnlyForDeploymentPostprocessing", Field::Str("0".into())),
            ],
            one_line: false,
        });
        // Xcode's order: Sources, Frameworks, Resources.
        let position = match kind {
            PhaseKind::Sources => target.phases.first().map_or(Position::End, |p| Position::Before(p.id.clone())),
            _ => Position::End,
        };
        self.batch.add_to_list(&target.id, "buildPhases", &id, Some(kind.name().into()), position);
        self.new_phases.insert((target.id.clone(), kind.isa()), id.clone());
        Some((id, kind.name().to_string()))
    }

    /// Makes file reference `reference` a member of `target`. Returns false
    /// when the target can't build this kind of file.
    fn add_to_target(&mut self, reference: &str, name: &str, kind: PhaseKind, target_id: &str) -> bool {
        let Some(target) = self.files.target(target_id).cloned() else { return false };
        let Some((phase, phase_name)) = self.phase_for(&target, kind) else { return false };
        let already = target
            .phases
            .iter()
            .filter(|p| p.id == phase)
            .flat_map(|p| &p.files)
            .any(|bf| self.files.build_files.get(bf).map(String::as_str) == Some(reference) && !self.batch.is_removed(bf));
        if already {
            return true;
        }
        let id = self.batch.ids.next();
        let comment = format!("{name} in {phase_name}");
        self.batch.add_object(NewObject {
            id: id.clone(),
            isa: "PBXBuildFile",
            comment: Some(comment.clone()),
            fields: vec![("fileRef", Field::Ref(reference.to_string(), Some(name.to_string())))],
            one_line: true,
        });
        self.batch.add_to_list(&phase, "files", &id, Some(comment), Position::End);
        self.changed = true;
        true
    }

    fn remove_from_target(&mut self, reference: &str, target_id: &str) {
        let builds: Vec<(String, String)> = self
            .files
            .memberships(reference)
            .into_iter()
            .filter(|(t, _, _)| t.id == target_id)
            .map(|(_, p, bf)| (p.id.clone(), bf.to_string()))
            .collect();
        for (phase, build_file) in builds {
            self.batch.remove_from_list(&phase, "files", &build_file);
            self.batch.remove_object(&build_file);
            self.changed = true;
        }
    }

    /// Adds a file reference for `path` to `group`, or returns the existing one.
    fn add_reference(&mut self, path: &Path, group: &str, file_type: &str) -> Option<(String, String)> {
        if let Some(existing) = self.files.file_reference(path).filter(|r| !self.batch.is_removed(&r.id)) {
            return Some((existing.id.clone(), existing.display_name()?.to_string()));
        }
        let group_dir = self.group_dir(group)?;
        let file_name = path.file_name()?.to_string_lossy().to_string();
        let rel = relative_path(&group_dir, path).to_string_lossy().to_string();
        let mut fields = vec![("lastKnownFileType", Field::Str(file_type.to_string()))];
        if rel != file_name {
            fields.push(("name", Field::Str(file_name.clone())));
        }
        fields.push(("path", Field::Str(rel)));
        fields.push(("sourceTree", Field::Str("<group>".into())));
        let id = self.batch.ids.next();
        self.batch.add_object(NewObject {
            id: id.clone(),
            isa: "PBXFileReference",
            comment: Some(file_name.clone()),
            fields,
            one_line: true,
        });
        self.add_child(group, &id, &file_name);
        Some((id, file_name))
    }

    /// Adds `path` (a file, or a folder of files) to `group` and `targets`.
    fn add_into(&mut self, group: &str, path: &Path, targets: &[String], options: &AddOptions) {
        let is_dir = path.is_dir();
        if is_dir && !options.folder_references && !is_opaque_folder(path) {
            let existing = self.files.group_for_dir(path).map(|g| g.id.clone());
            let sub = match existing.or_else(|| self.new_groups.iter().find(|(d, _)| d == path).map(|(_, id)| id.clone())) {
                Some(id) => id,
                None => {
                    let Some(group_dir) = self.group_dir(group) else { return };
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let rel = relative_path(&group_dir, path).to_string_lossy().to_string();
                    let mut fields = vec![("children", Field::List(vec![]))];
                    if rel != name {
                        fields.push(("name", Field::Str(name.clone())));
                    }
                    fields.push(("path", Field::Str(rel)));
                    fields.push(("sourceTree", Field::Str("<group>".into())));
                    let id = self.batch.ids.next();
                    self.batch.add_object(NewObject { id: id.clone(), isa: "PBXGroup", comment: Some(name.clone()), fields, one_line: false });
                    self.add_child(group, &id, &name);
                    self.new_groups.push((path.to_path_buf(), id.clone()));
                    id
                }
            };
            let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
                .map(|dir| dir.filter_map(|e| e.ok()).map(|e| e.path()).collect())
                .unwrap_or_default();
            entries.retain(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')));
            entries.sort();
            for entry in entries {
                self.add_into(&sub, &entry, targets, options);
            }
            return;
        }
        let file_type = file_type_for(path, is_dir);
        let Some((reference, name)) = self.add_reference(path, group, file_type) else { return };
        let file_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if let Some(kind) = PhaseKind::for_file(file_type, &file_name) {
            for target in targets {
                self.add_to_target(&reference, &name, kind, target);
            }
        }
    }

    /// Removes a group or file reference, its build files and everything
    /// under it.
    fn remove_item(&mut self, id: &str) {
        if id == self.files.main_group || self.files.products_group.as_deref() == Some(id) || self.batch.is_removed(id) {
            return;
        }
        let Some(item) = self.files.items.get(id).cloned() else { return };
        for child in &item.children {
            self.remove_item(child);
        }
        let targets: Vec<String> = self.files.memberships(id).iter().map(|(t, _, _)| t.id.clone()).collect();
        for target in targets {
            self.remove_from_target(id, &target);
        }
        if let Some(parent) = &item.parent {
            self.batch.remove_from_list(parent, "children", id);
        }
        let configs: Vec<String> = self
            .batch
            .objects()
            .entries()
            .iter()
            .filter(|e| e.value.get_str("baseConfigurationReference") == Some(id))
            .map(|e| e.key.clone())
            .collect();
        for config in configs {
            self.batch.remove_key(&config, "baseConfigurationReference");
        }
        self.batch.remove_object(id);
        self.changed = true;
    }

    /// Points comments for `id` (and its build files) at a new name.
    fn rename_comments(&mut self, id: &str, old: &str, new: &str) {
        if old == new {
            return;
        }
        self.batch.rename_comments(id, |c| (c == old).then(|| new.to_string()));
        let build_files: Vec<String> = self.files.memberships(id).iter().map(|(_, _, bf)| bf.to_string()).collect();
        for build_file in build_files {
            self.batch.rename_comments(&build_file, |c| {
                c.strip_prefix(old).filter(|rest| rest.starts_with(" in ")).map(|rest| format!("{new}{rest}"))
            });
        }
        self.changed = true;
    }
}

/// Adds files (and folders) to the project and to `targets`, creating groups
/// for their folders. Paths inside synchronized folders are skipped. Returns
/// `None` when nothing changed.
pub fn add_files(xcodeproj: &Path, text: &str, paths: &[PathBuf], targets: &[String], options: &AddOptions) -> Result<Option<String>> {
    let mut editor = Editor::new(xcodeproj, text)?;
    for path in paths {
        let path = canonical(path);
        if editor.files.synchronized_root(&path).is_some() {
            continue;
        }
        let group = match path.parent().and_then(|dir| editor.ensure_group(dir)) {
            Some(group) => group,
            None => {
                let destination = options.destination.as_deref().and_then(|d| editor.ensure_group(&canonical(d)));
                destination.unwrap_or_else(|| editor.files.main_group.clone())
            }
        };
        editor.add_into(&group, &path, targets, options);
    }
    editor.finish()
}

/// Removes every reference to `paths` (files or folders) and their build
/// files, as Move to Trash does in Xcode.
pub fn remove_paths(xcodeproj: &Path, text: &str, paths: &[PathBuf]) -> Result<Option<String>> {
    let mut editor = Editor::new(xcodeproj, text)?;
    for path in paths {
        let path = canonical(path);
        if editor.files.synchronized_root(&path).is_some() {
            continue;
        }
        let doomed: Vec<String> = editor
            .files
            .in_order()
            .filter(|i| i.isa != SYNCHRONIZED_GROUP && i.resolved.as_deref().is_some_and(|r| r.starts_with(&path)))
            .map(|i| i.id.clone())
            .collect();
        for id in doomed {
            editor.remove_item(&id);
        }
    }
    editor.finish()
}

/// Updates references after a file or folder moved from `from` to `to`
/// (renamed in place or moved to another folder).
pub fn move_path(xcodeproj: &Path, text: &str, from: &Path, to: &Path) -> Result<Option<String>> {
    let (from, to) = (canonical(from), canonical(to));
    let mut editor = Editor::new(xcodeproj, text)?;
    match (editor.files.synchronized_root(&from).is_some(), editor.files.synchronized_root(&to).is_some()) {
        (true, true) => return Ok(None),
        // Into a synchronized folder: the folder takes over.
        (false, true) => return remove_paths(xcodeproj, text, &[from]),
        // Out of one: reference it where it landed.
        (true, false) => {
            let targets = editor.files.suggested_targets(&to);
            return add_files(xcodeproj, text, &[to], &targets, &AddOptions::default());
        }
        (false, false) => {}
    }

    let under = |p: &Path, root: &Path| p.starts_with(root);
    let moved = |p: &Path| to.join(p.strip_prefix(&from).unwrap_or(Path::new("")));
    let items: Vec<Item> = editor
        .files
        .in_order()
        .filter(|i| i.id != editor.files.main_group && i.isa != SYNCHRONIZED_GROUP && i.path.is_some())
        .cloned()
        .collect();
    let same_folder = from.parent() == to.parent();
    for item in items {
        let (Some(base), Some(resolved)) = (item.base.clone(), item.resolved.clone()) else { continue };
        let base_moves = under(&base, &from);
        let item_moves = under(&resolved, &from);
        if base_moves == item_moves {
            // Moves with its group, or not at all.
            continue;
        }
        let old_display = item.display_name().unwrap_or("").to_string();
        let (new_base, new_resolved) = if base_moves {
            // Its group moved but it points outside it (`../x`).
            (moved(&base), resolved.clone())
        } else {
            (base.clone(), moved(&resolved))
        };
        let mut new_base = new_base;
        // A move to another folder regroups it under that folder's group.
        if !base_moves
            && !same_folder
            && item.source_tree == "<group>"
            && let Some(parent) = item.parent.clone()
            && let Some(group) = new_resolved.parent().and_then(|d| editor.ensure_group(d))
            && group != parent
            && !under(&editor.group_dir(&group).unwrap_or_default(), &from)
        {
            editor.batch.remove_from_list(&parent, "children", &item.id);
            let name = new_resolved.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            editor.add_child(&group, &item.id, &name);
            new_base = editor.group_dir(&group).unwrap_or(new_base);
        }
        let rel = if item.source_tree == "<absolute>" {
            new_resolved.to_string_lossy().to_string()
        } else {
            relative_path(&new_base, &new_resolved).to_string_lossy().to_string()
        };
        editor.batch.set_value(&item.id, "path", &rel);
        let old_file_name = resolved.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let new_file_name = new_resolved.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let new_name = match &item.name {
            Some(name) if *name == old_file_name => {
                editor.batch.set_value(&item.id, "name", &new_file_name);
                Some(new_file_name.clone())
            }
            None if rel.contains('/') && !item.is_group() => {
                editor.batch.set_value(&item.id, "name", &new_file_name);
                Some(new_file_name.clone())
            }
            other => other.clone(),
        };
        let new_display = new_name.unwrap_or_else(|| rel.clone());
        editor.rename_comments(&item.id, &old_display, &new_display);
        editor.changed = true;

        // A new extension may mean a new file type and build phase.
        if !item.is_group()
            && let Some(old_type) = item.file_type.clone()
            && item.isa == "PBXFileReference"
        {
            let new_type = file_type_for(&new_resolved, new_resolved.is_dir());
            if new_type != old_type && old_file_name.rsplit_once('.').map(|p| p.1) != new_file_name.rsplit_once('.').map(|p| p.1) {
                let key = if editor.batch.object(&item.id).and_then(|o| o.get("lastKnownFileType")).is_some() {
                    "lastKnownFileType"
                } else {
                    "explicitFileType"
                };
                editor.batch.set_value(&item.id, key, new_type);
                let (old_kind, new_kind) =
                    (PhaseKind::for_file(&old_type, &old_file_name), PhaseKind::for_file(new_type, &new_file_name));
                if old_kind != new_kind {
                    let targets: Vec<String> = editor
                        .files
                        .memberships(&item.id)
                        .iter()
                        .filter(|(_, p, _)| old_kind.is_some_and(|k| p.isa == k.isa()))
                        .map(|(t, _, _)| t.id.clone())
                        .collect();
                    for target in targets {
                        editor.remove_from_target(&item.id, &target);
                        if let Some(kind) = new_kind {
                            editor.add_to_target(&item.id, &new_display, kind, &target);
                        }
                    }
                }
            }
        }
    }
    editor.finish()
}

/// Adds `path` to or removes it from `target`, adding a file reference first
/// if the project has none.
pub fn set_target_membership(xcodeproj: &Path, text: &str, path: &Path, target: &str, member: bool) -> Result<Option<String>> {
    let path = canonical(path);
    let mut editor = Editor::new(xcodeproj, text)?;
    if editor.files.synchronized_root(&path).is_some() {
        return Ok(None);
    }
    let reference = editor.files.file_reference(&path).cloned();
    match (reference, member) {
        (Some(reference), true) => {
            let file_type = reference.file_type.clone().unwrap_or_else(|| file_type_for(&path, path.is_dir()).to_string());
            let file_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let kind = PhaseKind::for_file(&file_type, &file_name).ok_or_else(|| anyhow!("“{file_name}” isn’t built by targets"))?;
            let name = reference.display_name().unwrap_or(&file_name).to_string();
            if !editor.add_to_target(&reference.id, &name, kind, target) {
                return Err(anyhow!("the target can’t build “{file_name}”"));
            }
        }
        (Some(reference), false) => editor.remove_from_target(&reference.id, target),
        (None, true) => return add_files(xcodeproj, text, &[path], &[target.to_string()], &AddOptions::default()),
        (None, false) => {}
    }
    editor.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLASSIC: &str = include_str!("../../samples/HelloClassic/HelloClassic.xcodeproj/project.pbxproj");
    const SYNCED: &str = include_str!("../../samples/HelloZ/HelloZ.xcodeproj/project.pbxproj");
    const LEGACY: &str = include_str!("../../tests/fixtures/Legacy.pbxproj");
    const APP: &str = "7C1E5A0029F0B1C200D4E8F1";
    const TESTS: &str = "7C1E5A0F29F0B1C200D4E8F1";
    const LEGACY_APP: &str = "4F2E002C1B8C9D7000F1A2B3";
    const LEGACY_KIT: &str = "4F2E002D1B8C9D7000F1A2B3";
    const LEGACY_TOOL: &str = "4F2E002E1B8C9D7000F1A2B3";

    fn sample(name: &str) -> PathBuf {
        canonical(&Path::new(env!("CARGO_MANIFEST_DIR")).join("samples").join(name))
    }

    fn classic() -> (PathBuf, PathBuf) {
        let dir = sample("HelloClassic");
        (dir.join("HelloClassic.xcodeproj"), dir)
    }

    fn legacy() -> (PathBuf, PathBuf) {
        let dir = PathBuf::from("/zcode-fixture/Legacy");
        (dir.join("Legacy.xcodeproj"), dir)
    }

    /// Checks every original line survives, in order, and returns the new ones.
    fn inserted(original: &str, edited: &str) -> Vec<String> {
        let mut rest = original.lines().peekable();
        let mut added = vec![];
        for line in edited.lines() {
            if rest.peek() == Some(&line) {
                rest.next();
            } else {
                added.push(line.trim().to_string());
            }
        }
        assert_eq!(rest.next(), None, "an original line was changed or removed:\n{edited}");
        added
    }

    fn object_ids(text: &str) -> Vec<String> {
        super::super::parse(text).unwrap().get("objects").unwrap().entries().iter().map(|e| e.key.clone()).collect()
    }

    #[test]
    fn indexes_groups_and_membership() {
        let (proj, dir) = classic();
        let files = ProjectFiles::parse(&proj, CLASSIC).unwrap();
        let model = files.group_for_dir(&dir.join("HelloClassic/Model")).unwrap();
        assert_eq!(model.display_name(), Some("Model"));
        let greeter = files.file_reference(&dir.join("HelloClassic/Model/Greeter.swift")).unwrap();
        let members = files.memberships(&greeter.id);
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].0.name, "HelloClassic");
        assert_eq!(members[0].1.isa, "PBXSourcesBuildPhase");
        assert_eq!(files.group_for_dir(&dir).unwrap().id, files.main_group);
        assert!(files.synchronized_root(&dir.join("HelloClassic/New.swift")).is_none());
    }

    #[test]
    fn suggests_targets_from_neighbors() {
        let (proj, dir) = classic();
        let files = ProjectFiles::parse(&proj, CLASSIC).unwrap();
        assert_eq!(files.suggested_targets(&dir.join("HelloClassic/New.swift")), [APP]);
        assert_eq!(files.suggested_targets(&dir.join("HelloClassic/Model/New.swift")), [APP]);
        assert_eq!(files.suggested_targets(&dir.join("HelloClassic/Views/Deep/New.swift")), [APP]);
        assert_eq!(files.suggested_targets(&dir.join("HelloClassicTests/NewTests.swift")), [TESTS]);
        assert_eq!(files.suggested_targets(&dir.join("HelloClassic/data.json")), [APP]);
        assert!(files.suggested_targets(&dir.join("HelloClassic/Base.xcconfig")).is_empty());
    }

    #[test]
    fn adds_a_file_like_xcode() {
        let (proj, dir) = classic();
        let path = dir.join("HelloClassic/Model/Weather.swift");
        let out = add_files(&proj, CLASSIC, &[path.clone()], &[APP.into()], &AddOptions::default()).unwrap().unwrap();
        let added = inserted(CLASSIC, &out);
        assert_eq!(added.len(), 4, "{added:#?}");
        let files = ProjectFiles::parse(&proj, &out).unwrap();
        let reference = files.file_reference(&path).unwrap();
        let id = &reference.id;
        assert_eq!(id.len(), 24);
        assert!(id.bytes().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b)));
        assert!(added.contains(&format!(
            "{id} /* Weather.swift */ = {{isa = PBXFileReference; lastKnownFileType = sourcecode.swift; path = Weather.swift; sourceTree = \"<group>\"; }};"
        )));
        assert!(added.contains(&format!("{id} /* Weather.swift */,")));
        let (target, phase, build_file) = files.memberships(id)[0];
        assert_eq!((target.id.as_str(), phase.isa.as_str()), (APP, "PBXSourcesBuildPhase"));
        assert!(added.contains(&format!(
            "{build_file} /* Weather.swift in Sources */ = {{isa = PBXBuildFile; fileRef = {id} /* Weather.swift */; }};"
        )));
        assert!(added.contains(&format!("{build_file} /* Weather.swift in Sources */,")));
        // The Model group was sorted, so the file goes after Greeter.swift.
        assert!(out.contains(&format!("/* Greeter.swift */,\n\t\t\t\t{id} /* Weather.swift */,\n\t\t\t);")));
        // New objects are in their sections, in ID order.
        let ids = object_ids(&out);
        assert_eq!(ids.len(), object_ids(CLASSIC).len() + 2);
        // Adding it again changes nothing; removing it restores the file.
        assert_eq!(add_files(&proj, &out, &[path.clone()], &[APP.into()], &AddOptions::default()).unwrap(), None);
        assert_eq!(remove_paths(&proj, &out, &[path]).unwrap().unwrap(), CLASSIC);
    }

    #[test]
    fn creates_groups_for_new_folders() {
        let (proj, dir) = classic();
        let path = dir.join("HelloClassic/Views/Rows/Row.swift");
        let out = add_files(&proj, CLASSIC, &[path.clone()], &[APP.into()], &AddOptions::default()).unwrap().unwrap();
        let files = ProjectFiles::parse(&proj, &out).unwrap();
        let rows = files.group_for_dir(&dir.join("HelloClassic/Views/Rows")).unwrap();
        let views = files.group_for_dir(&dir.join("HelloClassic/Views")).unwrap();
        assert_eq!(rows.path.as_deref(), Some("Rows"));
        assert_eq!(rows.parent.as_deref(), Some(views.id.as_str()));
        assert_eq!(files.items[views.parent.as_ref().unwrap()].display_name(), Some("HelloClassic"));
        assert!(out.contains(&format!(
            "\t\t{} /* Views */ = {{\n\t\t\tisa = PBXGroup;\n\t\t\tchildren = (\n\t\t\t\t{} /* Rows */,\n\t\t\t);\n\t\t\tpath = Views;\n\t\t\tsourceTree = \"<group>\";\n\t\t}};\n",
            views.id, rows.id
        )));
        inserted(CLASSIC, &out);
        assert_eq!(remove_paths(&proj, &out, &[dir.join("HelloClassic/Views")]).unwrap().unwrap(), CLASSIC);
    }

    #[test]
    fn renames_and_moves_files() {
        let (proj, dir) = classic();
        let (old, new) = (dir.join("HelloClassic/Model/Greeter.swift"), dir.join("HelloClassic/Model/Greeting.swift"));
        let out = move_path(&proj, CLASSIC, &old, &new).unwrap().unwrap();
        assert!(!out.contains("Greeter.swift"));
        assert!(out.contains("/* Greeting.swift in Sources */ = {isa = PBXBuildFile; fileRef = 7C1E5A0A29F0B1C200D4E8F1 /* Greeting.swift */; };"));
        assert!(out.contains("path = Greeting.swift;"));
        assert_eq!(out.len(), CLASSIC.len() + 6);
        assert_eq!(move_path(&proj, &out, &new, &old).unwrap().unwrap(), CLASSIC);

        // To another folder: it changes groups and keeps its targets.
        let moved = dir.join("HelloClassic/Greeter.swift");
        let out = move_path(&proj, CLASSIC, &old, &moved).unwrap().unwrap();
        let files = ProjectFiles::parse(&proj, &out).unwrap();
        let reference = files.file_reference(&moved).unwrap();
        assert_eq!(files.items[reference.parent.as_ref().unwrap()].display_name(), Some("HelloClassic"));
        assert_eq!(files.memberships(&reference.id).len(), 1);
        assert_eq!(move_path(&proj, &out, &moved, &old).unwrap().unwrap(), CLASSIC);

        // Into a folder the project has no group for yet.
        let deeper = dir.join("HelloClassic/Model/Core/Greeter.swift");
        let out = move_path(&proj, CLASSIC, &old, &deeper).unwrap().unwrap();
        let files = ProjectFiles::parse(&proj, &out).unwrap();
        assert!(files.file_reference(&deeper).is_some());
        assert_eq!(files.group_for_dir(&dir.join("HelloClassic/Model/Core")).unwrap().path.as_deref(), Some("Core"));
    }

    #[test]
    fn renames_folders() {
        let (proj, dir) = classic();
        let (old, new) = (dir.join("HelloClassic/Model"), dir.join("HelloClassic/Models"));
        let out = move_path(&proj, CLASSIC, &old, &new).unwrap().unwrap();
        assert!(out.contains("7C1E5A0C29F0B1C200D4E8F1 /* Models */,"));
        assert!(out.contains("path = Models;"));
        let files = ProjectFiles::parse(&proj, &out).unwrap();
        assert!(files.file_reference(&new.join("Greeter.swift")).is_some());
        assert_eq!(move_path(&proj, &out, &new, &old).unwrap().unwrap(), CLASSIC);
        // Untracked paths change nothing.
        assert_eq!(move_path(&proj, CLASSIC, &dir.join("notes.txt"), &dir.join("notes.md")).unwrap(), None);
    }

    #[test]
    fn toggles_target_membership() {
        let (proj, dir) = classic();
        let greeter = dir.join("HelloClassic/Model/Greeter.swift");
        let out = set_target_membership(&proj, CLASSIC, &greeter, TESTS, true).unwrap().unwrap();
        let added = inserted(CLASSIC, &out);
        assert_eq!(added.len(), 2, "{added:#?}");
        let files = ProjectFiles::parse(&proj, &out).unwrap();
        let reference = files.file_reference(&greeter).unwrap();
        assert_eq!(files.memberships(&reference.id).len(), 2);
        assert_eq!(set_target_membership(&proj, &out, &greeter, TESTS, true).unwrap(), None);
        assert_eq!(set_target_membership(&proj, &out, &greeter, TESTS, false).unwrap().unwrap(), CLASSIC);
        // Checking a file the project doesn't reference yet adds it.
        let loose = dir.join("HelloClassic/Loose.swift");
        let out = set_target_membership(&proj, CLASSIC, &loose, APP, true).unwrap().unwrap();
        assert!(ProjectFiles::parse(&proj, &out).unwrap().file_reference(&loose).is_some());
    }

    #[test]
    fn file_info_for_the_inspector() {
        let (proj, dir) = classic();
        let files = ProjectFiles::parse(&proj, CLASSIC).unwrap();
        let info = files.file_info(&dir.join("HelloClassic/Model/Greeter.swift"));
        assert_eq!(info.file_type, "sourcecode.swift");
        assert_eq!(info.location, "Relative to Group");
        let Tracking::Referenced { referenced: true, targets } = info.tracking else { panic!("{info:?}") };
        assert_eq!(targets.iter().map(|t| (t.name.as_str(), t.member, t.available)).collect::<Vec<_>>(), [
            ("HelloClassic", true, true),
            ("HelloClassicTests", false, true)
        ]);
        assert_eq!(files.file_info(&dir.join("HelloClassic/Model")).tracking, Tracking::Group);

        let synced = sample("HelloZ");
        let files = ProjectFiles::parse(&synced.join("HelloZ.xcodeproj"), SYNCED).unwrap();
        let info = files.file_info(&synced.join("HelloZ/Counter.swift"));
        assert_eq!(info.tracking, Tracking::Synchronized { folder: "HelloZ".into(), targets: vec!["HelloZ".into()] });
    }

    #[test]
    fn leaves_synchronized_folders_alone() {
        let dir = sample("HelloZ");
        let proj = dir.join("HelloZ.xcodeproj");
        let file = dir.join("HelloZ/New.swift");
        assert_eq!(add_files(&proj, SYNCED, &[file.clone()], &["A10000000000000000000006".into()], &AddOptions::default()).unwrap(), None);
        assert_eq!(remove_paths(&proj, SYNCED, &[dir.join("HelloZ/Counter.swift")]).unwrap(), None);
        assert_eq!(move_path(&proj, SYNCED, &dir.join("HelloZ/Counter.swift"), &dir.join("HelloZ/Count.swift")).unwrap(), None);
        assert_eq!(set_target_membership(&proj, SYNCED, &dir.join("HelloZ/Counter.swift"), "A10000000000000000000016", true).unwrap(), None);
    }

    #[test]
    fn creates_missing_sections() {
        // HelloZ has no PBXBuildFile section; a file outside its synchronized
        // folders still goes in the classic way.
        let dir = sample("HelloZ");
        let proj = dir.join("HelloZ.xcodeproj");
        let readme = dir.join("Notes.md");
        let out = add_files(&proj, SYNCED, &[readme.clone()], &["A10000000000000000000006".into()], &AddOptions::default())
            .unwrap()
            .unwrap();
        inserted(SYNCED, &out);
        let begin = out.find("/* Begin PBXBuildFile section */").unwrap();
        assert!(begin < out.find("/* Begin PBXContainerItemProxy section */").unwrap());
        assert!(out[begin..].starts_with("/* Begin PBXBuildFile section */\n\t\t"));
        assert!(out.contains("/* End PBXBuildFile section */\n\n/* Begin PBXContainerItemProxy section */"));
        assert!(out.contains("/* Notes.md in Resources */,\n\t\t\t);"));
        assert_eq!(remove_paths(&proj, &out, &[readme]).unwrap().unwrap(), SYNCED);
    }

    #[test]
    fn legacy_projects() {
        let (proj, dir) = legacy();
        let files = ProjectFiles::parse(&proj, LEGACY).unwrap();
        // A virtual group shares its parent's folder.
        let main = files.file_reference(&dir.join("Legacy/main.m")).unwrap();
        assert_eq!(files.items[main.parent.as_ref().unwrap()].display_name(), Some("Supporting Files"));
        assert_eq!(files.group_for_dir(&dir.join("Legacy")).unwrap().display_name(), Some("Legacy"));
        assert!(files.file_reference(&dir.join("Config/Base.xcconfig")).is_some());

        // Headers go to the framework's Headers phase; tools take no resources.
        let header = dir.join("LegacyKit/LKOther.h");
        let out = add_files(&proj, LEGACY, &[header], &[LEGACY_KIT.into(), LEGACY_APP.into()], &AddOptions::default()).unwrap().unwrap();
        let added = inserted(LEGACY, &out);
        assert!(added.iter().any(|l| l.ends_with("/* LKOther.h in Headers */,")), "{added:#?}");
        assert_eq!(added.iter().filter(|l| l.contains("in Headers")).count(), 2, "{added:#?}");
        let json = dir.join("legacytool/config.json");
        let files = ProjectFiles::parse(&proj, LEGACY).unwrap();
        assert!(!files.target(LEGACY_TOOL).unwrap().accepts(PhaseKind::Resources));
        let out = add_files(&proj, LEGACY, &[json], &[LEGACY_TOOL.into()], &AddOptions::default()).unwrap().unwrap();
        assert!(!out.contains("config.json in"));

        // Trashing the xcconfig drops the configurations' references to it.
        let out = remove_paths(&proj, LEGACY, &[dir.join("Config/Base.xcconfig")]).unwrap().unwrap();
        assert!(!out.contains("Base.xcconfig"));
        assert!(!out.contains("baseConfigurationReference"));
        super::super::parse(&out).unwrap();

        // Trashing a folder removes its group, files and build files, and
        // build phases keep their other files.
        let out = remove_paths(&proj, LEGACY, &[dir.join("LegacyKit")]).unwrap().unwrap();
        for gone in ["LKThing", "LegacyKit.h", "4F2E001F1B8C9D7000F1A2B3"] {
            assert!(!out.contains(gone), "{gone} left in:\n{out}");
        }
        assert!(out.contains("/* Begin PBXHeadersBuildPhase section */"), "the phase itself stays");
        assert!(out.contains("LegacyKit.framework in Frameworks"));

        // Renaming data.json to a Swift file moves it from Resources to Sources.
        let out = move_path(&proj, LEGACY, &dir.join("Legacy/data.json"), &dir.join("Legacy/Data.swift")).unwrap().unwrap();
        let files = ProjectFiles::parse(&proj, &out).unwrap();
        let reference = files.file_reference(&dir.join("Legacy/Data.swift")).unwrap();
        assert_eq!(reference.file_type.as_deref(), Some("sourcecode.swift"));
        let phases: Vec<_> = files.memberships(&reference.id).iter().map(|(_, p, _)| p.isa.clone()).collect();
        assert_eq!(phases, ["PBXSourcesBuildPhase"]);
        assert!(out.contains("/* Data.swift in Sources */"));
        assert!(!out.contains("data.json"));

        // A file in a virtual group renames inside it.
        let out = move_path(&proj, LEGACY, &dir.join("Legacy/main.m"), &dir.join("Legacy/start.m")).unwrap().unwrap();
        assert!(out.contains("/* start.m in Sources */ = {isa = PBXBuildFile;"));
        let files = ProjectFiles::parse(&proj, &out).unwrap();
        let start = files.file_reference(&dir.join("Legacy/start.m")).unwrap();
        assert_eq!(files.items[start.parent.as_ref().unwrap()].display_name(), Some("Supporting Files"));
    }

    #[test]
    fn adds_folders_as_groups_or_references() {
        let tmp = std::env::temp_dir().join(format!("zcode-files-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("App.xcodeproj")).unwrap();
        std::fs::create_dir_all(tmp.join("App/Feature/Nested")).unwrap();
        std::fs::create_dir_all(tmp.join("App/Feature/Media.xcassets")).unwrap();
        for file in ["App/Feature/A.swift", "App/Feature/b.png", "App/Feature/Nested/C.swift", "App/Feature/.DS_Store"] {
            std::fs::write(tmp.join(file), "").unwrap();
        }
        let proj = tmp.join("App.xcodeproj");
        let text = CLASSIC.replace("path = HelloClassic;", "path = App;");
        let feature = tmp.join("App/Feature");
        let out = add_files(&proj, &text, &[feature.clone()], &[APP.into()], &AddOptions::default()).unwrap().unwrap();
        let files = ProjectFiles::parse(&proj, &out).unwrap();
        let feature = canonical(&feature);
        for (file, phase) in [("A.swift", "PBXSourcesBuildPhase"), ("b.png", "PBXResourcesBuildPhase"), ("Nested/C.swift", "PBXSourcesBuildPhase"), ("Media.xcassets", "PBXResourcesBuildPhase")] {
            let reference = files.file_reference(&feature.join(file)).unwrap_or_else(|| panic!("{file} not added"));
            assert_eq!(files.memberships(&reference.id)[0].1.isa, phase, "{file}");
        }
        assert!(!out.contains(".DS_Store"));
        assert!(files.group_for_dir(&feature.join("Nested")).is_some());

        let out = add_files(&proj, &text, &[feature.clone()], &[APP.into()], &AddOptions { folder_references: true, ..Default::default() }).unwrap().unwrap();
        assert!(out.contains("/* Feature */ = {isa = PBXFileReference; lastKnownFileType = folder; path = Feature; sourceTree = \"<group>\"; };"));
        assert!(out.contains("/* Feature in Resources */"));

        // Files outside the project folder are referenced where they are.
        let outside = std::env::temp_dir().join(format!("zcode-files-outside-{}.swift", std::process::id()));
        std::fs::write(&outside, "").unwrap();
        let out = add_files(&proj, &text, &[outside.clone()], &[APP.into()], &AddOptions::default()).unwrap().unwrap();
        let files = ProjectFiles::parse(&proj, &out).unwrap();
        let reference = files.file_reference(&canonical(&outside)).unwrap();
        assert!(reference.path.as_deref().unwrap().starts_with("../"));
        assert_eq!(reference.name.as_deref(), outside.file_name().and_then(|n| n.to_str()));
        let _ = std::fs::remove_file(outside);
        let _ = std::fs::remove_dir_all(tmp);
    }
}

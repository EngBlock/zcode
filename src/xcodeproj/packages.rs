//! Swift package dependencies in `project.pbxproj`, edited the way Xcode's
//! Add Package Dependencies and Remove Package do: an
//! `XCRemoteSwiftPackageReference` (URL and dependency rule) or
//! `XCLocalSwiftPackageReference` (relative path) in the project's
//! `packageReferences`, and for each product a target links an
//! `XCSwiftPackageProductDependency` in the target's
//! `packageProductDependencies` plus a `PBXBuildFile` with a `productRef`
//! in its Frameworks build phase.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use super::Node;
use super::edit::{Batch, Field, NewObject, Position};
use super::files::{PhaseKind, canonical};
use crate::packages::resolved::display_name;
use crate::packages::rule::Rule;
use crate::paths::relative_path;

pub const REMOTE: &str = "XCRemoteSwiftPackageReference";
pub const LOCAL: &str = "XCLocalSwiftPackageReference";
pub const PRODUCT: &str = "XCSwiftPackageProductDependency";

#[derive(Clone, Debug, PartialEq)]
pub struct PackageReference {
    pub id: String,
    pub local: bool,
    /// The repository URL, or the path relative to the project's folder.
    pub location: String,
    pub rule: Option<Rule>,
}

impl PackageReference {
    pub fn name(&self) -> String {
        display_name(&self.location)
    }

    /// `XCRemoteSwiftPackageReference "swift-collections"`, as Xcode
    /// comments references.
    pub fn comment(&self) -> String {
        if self.local {
            format!("{LOCAL} \"{}\"", self.location)
        } else {
            format!("{REMOTE} \"{}\"", self.name())
        }
    }

    /// Where a local package is, given the `.xcodeproj` that refers to it.
    pub fn local_path(&self, xcodeproj: &Path) -> Option<PathBuf> {
        self.local.then(|| super::files::normalize(&xcodeproj.parent().unwrap_or(Path::new("/")).join(&self.location)))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProductDependency {
    pub id: String,
    pub product: String,
    /// The package reference, absent in projects from before Xcode 15 that
    /// linked local packages by name.
    pub package: Option<String>,
    /// IDs of the targets that list it.
    pub targets: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct PackageTarget {
    pub id: String,
    pub name: String,
    pub product_type: Option<String>,
}

/// The package references, product dependencies and native targets of one
/// project.
#[derive(Clone, Debug, Default)]
pub struct ProjectPackages {
    pub references: Vec<PackageReference>,
    pub products: Vec<ProductDependency>,
    pub targets: Vec<PackageTarget>,
}

impl ProjectPackages {
    pub fn parse(text: &str) -> Result<Self> {
        Self::from_root(&super::parse(text)?)
    }

    pub fn from_root(root: &Node) -> Result<Self> {
        let objects = root.get("objects").ok_or_else(|| anyhow!("no objects"))?;
        let project_id = root.get_str("rootObject").ok_or_else(|| anyhow!("no rootObject"))?;
        let project = objects.get(project_id).ok_or_else(|| anyhow!("missing project object"))?;
        let ids = |node: Option<&Node>| -> Vec<String> {
            node.map(Node::items).unwrap_or_default().iter().filter_map(Node::as_str).map(str::to_string).collect()
        };
        let references = ids(project.get("packageReferences"))
            .into_iter()
            .filter_map(|id| {
                let object = objects.get(&id)?;
                let local = match object.get_str("isa")? {
                    REMOTE => false,
                    LOCAL => true,
                    _ => return None,
                };
                let location = object.get_str(if local { "relativePath" } else { "repositoryURL" })?.to_string();
                let rule = object.get("requirement").and_then(|r| Rule::from_pbxproj(|key| r.get_str(key)));
                Some(PackageReference { id, local, location, rule })
            })
            .collect();
        let target_ids = ids(project.get("targets"));
        let targets: Vec<PackageTarget> = target_ids
            .iter()
            .filter_map(|id| {
                let target = objects.get(id)?;
                (target.get_str("isa") == Some("PBXNativeTarget")).then(|| PackageTarget {
                    id: id.clone(),
                    name: target.get_str("name").unwrap_or(id).to_string(),
                    product_type: target.get_str("productType").map(str::to_string),
                })
            })
            .collect();
        let products = objects
            .entries()
            .iter()
            .filter(|e| e.value.get_str("isa") == Some(PRODUCT))
            .map(|e| ProductDependency {
                id: e.key.clone(),
                product: e.value.get_str("productName").unwrap_or_default().to_string(),
                package: e.value.get_str("package").map(str::to_string),
                targets: target_ids
                    .iter()
                    .filter(|t| objects.get(t).is_some_and(|t| ids(t.get("packageProductDependencies")).contains(&e.key)))
                    .cloned()
                    .collect(),
            })
            .collect();
        Ok(Self { references, products, targets })
    }

    pub fn reference(&self, id: &str) -> Option<&PackageReference> {
        self.references.iter().find(|r| r.id == id)
    }

    /// The products of `reference` each target links, as (product, target name).
    pub fn linked(&self, reference: &str) -> Vec<(String, String)> {
        let mut out = vec![];
        for product in self.products.iter().filter(|p| p.package.as_deref() == Some(reference)) {
            for target in &product.targets {
                let name = self.targets.iter().find(|t| &t.id == target).map(|t| t.name.clone()).unwrap_or_default();
                out.push((product.product.clone(), name));
            }
        }
        out.sort();
        out
    }
}

/// Two repository URLs name the same package (case, a `.git` suffix and a
/// trailing slash aside).
pub fn same_url(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let s = s.trim().trim_end_matches('/').to_lowercase();
        s.strip_suffix(".git").map(str::to_string).unwrap_or(s)
    };
    norm(a) == norm(b)
}

/// What to add.
#[derive(Clone, Debug)]
pub enum Source {
    Remote { url: String, rule: Rule },
    /// A package folder on disk.
    Local { path: PathBuf },
}

struct PackageEditor<'a> {
    batch: Batch<'a>,
    packages: ProjectPackages,
    project_id: String,
    /// Frameworks phases created in this batch, by target.
    new_phases: Vec<(String, String)>,
}

impl<'a> PackageEditor<'a> {
    fn new(text: &'a str) -> Result<Self> {
        let batch = Batch::new(text)?;
        let packages = ProjectPackages::from_root(&batch.root)?;
        let project_id = batch.root.get_str("rootObject").ok_or_else(|| anyhow!("no rootObject"))?.to_string();
        Ok(Self { batch, packages, project_id, new_phases: vec![] })
    }

    /// The target's Frameworks phase, created (after Sources, as Xcode
    /// orders phases) when it has none.
    fn frameworks_phase(&mut self, target: &str) -> Option<String> {
        if let Some((_, id)) = self.new_phases.iter().find(|(t, _)| t == target) {
            return Some(id.clone());
        }
        let object = self.batch.object(target)?;
        let phases: Vec<String> =
            object.get("buildPhases").map(Node::items).unwrap_or_default().iter().filter_map(Node::as_str).map(str::to_string).collect();
        let isa_of = |id: &str| self.batch.object(id).and_then(|o| o.get_str("isa")).map(str::to_string);
        if let Some(existing) = phases.iter().find(|p| isa_of(p).as_deref() == Some(PhaseKind::Frameworks.isa())) {
            return Some(existing.clone());
        }
        let after_sources = phases
            .iter()
            .position(|p| isa_of(p).as_deref() == Some(PhaseKind::Sources.isa()))
            .and_then(|ix| phases.get(ix + 1))
            .map_or(Position::End, |next| Position::Before(next.clone()));
        let id = self.batch.ids.next();
        self.batch.add_object(NewObject {
            id: id.clone(),
            isa: PhaseKind::Frameworks.isa(),
            comment: Some(PhaseKind::Frameworks.name().into()),
            fields: vec![
                ("buildActionMask", Field::Str("2147483647".into())),
                ("files", Field::List(vec![])),
                ("runOnlyForDeploymentPostprocessing", Field::Str("0".into())),
            ],
            one_line: false,
        });
        self.batch.add_to_list(target, "buildPhases", &id, Some(PhaseKind::Frameworks.name().into()), after_sources);
        self.new_phases.push((target.to_string(), id.clone()));
        Some(id)
    }

    /// Links `product` of package `reference` (commented `comment`) into
    /// `target`, unless it already does.
    fn link(&mut self, reference: &str, comment: &str, product: &str, target: &str) -> Result<()> {
        if !self.packages.targets.iter().any(|t| t.id == target) {
            return Err(anyhow!("no target {target}"));
        }
        let linked = self.packages.products.iter().any(|p| {
            p.product == product && p.package.as_deref() == Some(reference) && p.targets.iter().any(|t| t == target)
        });
        if linked {
            return Ok(());
        }
        let dependency = self.batch.ids.next();
        self.batch.add_object(NewObject {
            id: dependency.clone(),
            isa: PRODUCT,
            comment: Some(product.to_string()),
            fields: vec![
                ("package", Field::Ref(reference.to_string(), Some(comment.to_string()))),
                ("productName", Field::Str(product.to_string())),
            ],
            one_line: false,
        });
        self.batch.add_to_list(target, "packageProductDependencies", &dependency, Some(product.to_string()), Position::End);
        let phase = self.frameworks_phase(target).ok_or_else(|| anyhow!("no target {target}"))?;
        let build_file = self.batch.ids.next();
        let build_comment = format!("{product} in {}", PhaseKind::Frameworks.name());
        self.batch.add_object(NewObject {
            id: build_file.clone(),
            isa: "PBXBuildFile",
            comment: Some(build_comment.clone()),
            fields: vec![("productRef", Field::Ref(dependency, Some(product.to_string())))],
            one_line: true,
        });
        self.batch.add_to_list(&phase, "files", &build_file, Some(build_comment), Position::End);
        Ok(())
    }
}

/// Adds a package to the project (or finds it there) and links each
/// `(product, target ID)`. Returns the new text and the package
/// reference's ID, or `None` for the text when nothing changed.
pub fn add_package(xcodeproj: &Path, text: &str, source: &Source, products: &[(String, String)]) -> Result<(Option<String>, String)> {
    let mut editor = PackageEditor::new(text)?;
    let project_dir = canonical(xcodeproj.parent().unwrap_or(Path::new("/")));
    let existing = editor.packages.references.iter().find(|r| match source {
        Source::Remote { url, .. } => !r.local && same_url(&r.location, url),
        Source::Local { path } => r.local_path(xcodeproj).is_some_and(|p| canonical(&p) == canonical(path)),
    });
    let mut changed = false;
    let reference = match existing {
        Some(reference) => reference.clone(),
        None => {
            let id = editor.batch.ids.next();
            let reference = match source {
                Source::Remote { url, rule } => PackageReference { id: id.clone(), local: false, location: url.clone(), rule: Some(rule.clone()) },
                Source::Local { path } => {
                    let rel = relative_path(&project_dir, &canonical(path)).to_string_lossy().to_string();
                    PackageReference { id: id.clone(), local: true, location: rel, rule: None }
                }
            };
            let fields = match (&reference.rule, reference.local) {
                (Some(rule), false) => vec![
                    ("repositoryURL", Field::Str(reference.location.clone())),
                    ("requirement", Field::Dict(rule.pbxproj_fields())),
                ],
                _ => vec![("relativePath", Field::Str(reference.location.clone()))],
            };
            editor.batch.add_object(NewObject {
                id: id.clone(),
                isa: if reference.local { LOCAL } else { REMOTE },
                comment: Some(reference.comment()),
                fields,
                one_line: false,
            });
            let project = editor.project_id.clone();
            editor.batch.add_to_list(&project, "packageReferences", &id, Some(reference.comment()), Position::End);
            changed = true;
            reference
        }
    };
    let comment = reference.comment();
    for (product, target) in products {
        let before = editor.batch.new_objects.len();
        editor.link(&reference.id, &comment, product, target)?;
        changed |= editor.batch.new_objects.len() != before;
    }
    let text = if changed { Some(editor.batch.finish()?) } else { None };
    Ok((text, reference.id))
}

/// Removes package reference `reference`, its product dependencies and
/// the build files that link them. `None` when the project doesn't have it.
pub fn remove_package(text: &str, reference: &str) -> Result<Option<String>> {
    let mut editor = PackageEditor::new(text)?;
    if editor.packages.reference(reference).is_none() {
        return Ok(None);
    }
    let project = editor.project_id.clone();
    let dependencies: Vec<ProductDependency> =
        editor.packages.products.iter().filter(|p| p.package.as_deref() == Some(reference)).cloned().collect();
    // Emptied lists stay, as Xcode leaves them.
    editor.batch.remove_from_list(&project, "packageReferences", reference);
    editor.batch.remove_object(reference);
    for dependency in &dependencies {
        for target in &dependency.targets {
            editor.batch.remove_from_list(target, "packageProductDependencies", &dependency.id);
        }
    }
    // Build files and the phases that list them.
    let objects: Vec<(String, Node)> = editor.batch.objects().entries().iter().map(|e| (e.key.clone(), e.value.clone())).collect();
    for dependency in &dependencies {
        editor.batch.remove_object(&dependency.id);
        for (build_file, object) in &objects {
            if object.get_str("isa") != Some("PBXBuildFile") || object.get_str("productRef") != Some(dependency.id.as_str()) {
                continue;
            }
            editor.batch.remove_object(build_file);
            for (phase, phase_object) in &objects {
                if phase_object.get("files").is_some_and(|f| f.items().iter().any(|i| i.as_str() == Some(build_file.as_str()))) {
                    editor.batch.remove_from_list(phase, "files", build_file);
                }
            }
        }
    }
    editor.batch.finish().map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLASSIC: &str = include_str!("../../samples/HelloClassic/HelloClassic.xcodeproj/project.pbxproj");
    const HELLOZ: &str = include_str!("../../samples/HelloZ/HelloZ.xcodeproj/project.pbxproj");
    const APP: &str = "7C1E5A0029F0B1C200D4E8F1";
    const TESTS: &str = "7C1E5A0F29F0B1C200D4E8F1";

    /// The text without empty `packageReferences` and
    /// `packageProductDependencies` lists, which removing a package leaves.
    fn without_empty_lists(text: &str) -> String {
        let mut out = text.to_string();
        for key in ["packageReferences", "packageProductDependencies"] {
            out = out.replace(&format!("\t\t\t{key} = (\n\t\t\t);\n"), "");
        }
        out
    }

    fn removed(text: &str, reference: &str) -> String {
        without_empty_lists(&remove_package(text, reference).unwrap().unwrap())
    }

    fn proj() -> PathBuf {
        PathBuf::from("/work/HelloClassic/HelloClassic.xcodeproj")
    }

    fn tally() -> Source {
        Source::Remote { url: "https://github.com/zcode/Tally.git".into(), rule: Rule::UpToNextMajor("1.0.0".into()) }
    }

    #[test]
    fn adds_a_remote_package_like_xcode() {
        let (out, id) = add_package(&proj(), CLASSIC, &tally(), &[("Tally".into(), APP.into()), ("TallyFormat".into(), TESTS.into())]).unwrap();
        let out = out.unwrap();
        // The reference, in its own section after the configuration lists.
        let reference = format!(
            "/* Begin XCRemoteSwiftPackageReference section */\n\t\t{id} /* XCRemoteSwiftPackageReference \"Tally\" */ = {{\n\t\t\tisa = XCRemoteSwiftPackageReference;\n\t\t\trepositoryURL = \"https://github.com/zcode/Tally.git\";\n\t\t\trequirement = {{\n\t\t\t\tkind = upToNextMajorVersion;\n\t\t\t\tminimumVersion = 1.0.0;\n\t\t\t}};\n\t\t}};\n/* End XCRemoteSwiftPackageReference section */\n"
        );
        assert!(out.contains(&reference), "{out}");
        let lists = out.find("/* End XCConfigurationList section */").unwrap();
        assert!(out.find("/* Begin XCRemoteSwiftPackageReference section */").unwrap() > lists);
        assert!(out.find("/* Begin XCSwiftPackageProductDependency section */").unwrap() > out.find("/* End XCRemoteSwiftPackageReference section */").unwrap());
        assert!(out.contains(&format!("\t\t\tpackageReferences = (\n\t\t\t\t{id} /* XCRemoteSwiftPackageReference \"Tally\" */,\n\t\t\t);\n")), "{out}");
        assert!(out.contains("/* Tally in Frameworks */ = {isa = PBXBuildFile; productRef = "), "{out}");
        assert!(out.contains(&format!("\t\t\tpackage = {id} /* XCRemoteSwiftPackageReference \"Tally\" */;\n\t\t\tproductName = Tally;\n")));

        let read = ProjectPackages::parse(&out).unwrap();
        assert_eq!(read.references.len(), 1);
        assert_eq!(read.references[0].rule, Some(Rule::UpToNextMajor("1.0.0".into())));
        assert_eq!(read.linked(&id), [("Tally".to_string(), "HelloClassic".to_string()), ("TallyFormat".to_string(), "HelloClassicTests".to_string())]);
        // Each target lists its product, and its Frameworks phase links it.
        let files = super::super::files::ProjectFiles::parse(&proj(), &out).unwrap();
        let app = files.target(APP).unwrap();
        assert_eq!(app.phase(PhaseKind::Frameworks).unwrap().files.len(), 1);

        // Only lines were inserted.
        let mut rest = CLASSIC.lines();
        let mut want = rest.next();
        for line in out.lines() {
            if Some(line) == want {
                want = rest.next();
            }
        }
        assert!(want.is_none(), "an original line changed: {want:?}");

        // Adding again links nothing new; removing restores the file.
        let (again, same) = add_package(&proj(), &out, &tally(), &[("Tally".into(), APP.into())]).unwrap();
        assert_eq!((again, same.as_str()), (None, id.as_str()));
        assert_eq!(removed(&out, &id), CLASSIC);
    }

    #[test]
    fn adds_products_to_a_package_already_there() {
        let (out, id) = add_package(&proj(), CLASSIC, &tally(), &[("Tally".into(), APP.into())]).unwrap();
        let out = out.unwrap();
        let (more, same) = add_package(&proj(), &out, &tally(), &[("TallyFormat".into(), APP.into())]).unwrap();
        assert_eq!(same, id);
        let more = more.unwrap();
        assert_eq!(more.matches("XCRemoteSwiftPackageReference;").count(), 1);
        let read = ProjectPackages::parse(&more).unwrap();
        assert_eq!(read.linked(&id).len(), 2);
        assert_eq!(removed(&more, &id), CLASSIC);
    }

    #[test]
    fn adds_a_local_package() {
        let source = Source::Local { path: PathBuf::from("/work/ZKit") };
        let (out, id) = add_package(&proj(), CLASSIC, &source, &[("ZKit".into(), APP.into())]).unwrap();
        let out = out.unwrap();
        assert!(out.contains(&format!(
            "\t\t{id} /* XCLocalSwiftPackageReference \"../ZKit\" */ = {{\n\t\t\tisa = XCLocalSwiftPackageReference;\n\t\t\trelativePath = ../ZKit;\n\t\t}};\n"
        )), "{out}");
        let read = ProjectPackages::parse(&out).unwrap();
        assert_eq!(read.references[0].local_path(&proj()).unwrap(), Path::new("/work/ZKit"));
        assert_eq!(removed(&out, &id), CLASSIC);
    }

    #[test]
    fn creates_frameworks_phases_and_lists() {
        // HelloZ's targets have Frameworks phases but no product lists, and
        // the project has no PBXBuildFile section.
        let proj = PathBuf::from("/work/HelloZ/HelloZ.xcodeproj");
        let packages = ProjectPackages::parse(HELLOZ).unwrap();
        let app = packages.targets[0].id.clone();
        let (out, id) = add_package(&proj, HELLOZ, &tally(), &[("Tally".into(), app.clone())]).unwrap();
        let out = out.unwrap();
        assert!(out.contains("/* Begin PBXBuildFile section */"));
        assert!(out.contains("\t\t\tpackageProductDependencies = (\n"), "{out}");
        assert_eq!(ProjectPackages::parse(&out).unwrap().linked(&id).len(), 1);
        assert_eq!(removed(&out, &id), without_empty_lists(HELLOZ));

        // A target without a Frameworks phase gets one after Sources.
        let text = HELLOZ.replace(
            &format!("{} /* Frameworks */,\n", frameworks_phase_of(HELLOZ, &app)),
            "",
        );
        let (out, _) = add_package(&proj, &text, &tally(), &[("Tally".into(), app.clone())]).unwrap();
        let out = out.unwrap();
        let files = super::super::files::ProjectFiles::parse(&proj, &out).unwrap();
        let target = files.target(&app).unwrap();
        let kinds: Vec<&str> = target.phases.iter().map(|p| p.isa.as_str()).collect();
        let sources = kinds.iter().position(|k| *k == "PBXSourcesBuildPhase").unwrap();
        assert_eq!(kinds[sources + 1], "PBXFrameworksBuildPhase", "{kinds:?}");
    }

    fn frameworks_phase_of(text: &str, target: &str) -> String {
        let root = super::super::parse(text).unwrap();
        let objects = root.get("objects").unwrap();
        objects
            .get(target)
            .unwrap()
            .get("buildPhases")
            .unwrap()
            .items()
            .iter()
            .filter_map(Node::as_str)
            .find(|p| objects.get(p).and_then(|o| o.get_str("isa")) == Some("PBXFrameworksBuildPhase"))
            .unwrap()
            .to_string()
    }

    #[test]
    fn compares_urls() {
        assert!(same_url("https://github.com/apple/swift-collections.git", "https://GitHub.com/apple/swift-collections/"));
        assert!(!same_url("https://github.com/apple/swift-collections", "https://github.com/apple/swift-algorithms"));
    }
}

//! Adds packages to a copy of the HelloClassic sample the way Add Package
//! Dependencies does (a remote package, served offline from a local bare
//! repository of the Tally fixture, and a local package), then checks
//! `xcodebuild -resolvePackageDependencies` resolves them, the app builds
//! against them, and Remove Package undoes it. Requires Xcode;
//! `cargo test -- --ignored`.

use std::path::{Path, PathBuf};
use std::process::Command;

use zcode::packages::{self, Operation, resolved, rule::Rule};
use zcode::project::{ProjectInfo, ProjectKind};
use zcode::xcodeproj::packages::{ProjectPackages, Source, add_package, remove_package};

const APP: &str = "7C1E5A0029F0B1C200D4E8F1";

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The Tally fixture as a bare git repository with release tags.
pub fn tally_repo(work: &Path) -> PathBuf {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/packages/make_repo.sh");
    let output = Command::new("/bin/sh").arg(script).arg(work).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
}

fn xcodebuild(proj: &Path, derived: &Path, args: &[&str]) -> (bool, String) {
    let output = Command::new("/usr/bin/xcrun")
        .args(["xcodebuild", "-project"])
        .arg(proj)
        // xcodebuild only takes a DerivedData folder with a scheme.
        .arg("-clonedSourcePackagesDirPath")
        .arg(derived.join("SourcePackages"))
        .args(args)
        .output()
        .unwrap();
    let mut log = String::from_utf8_lossy(&output.stdout).to_string();
    log.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), log)
}

#[test]
#[ignore]
fn xcodebuild_resolves_and_builds_added_packages() {
    let work = std::env::temp_dir().join(format!("zcode-packages-build-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let work = work.canonicalize().unwrap();
    let repo = tally_repo(&work);
    let url = format!("file://{}", repo.display());
    let samples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("samples");
    copy_dir(&samples.join("HelloClassic"), &work.join("HelloClassic"));
    copy_dir(&samples.join("ZKit"), &work.join("ZKit"));
    let proj = work.join("HelloClassic/HelloClassic.xcodeproj");
    let pbxproj = proj.join("project.pbxproj");
    let original = std::fs::read_to_string(&pbxproj).unwrap();

    // Tally 1.x, both products in the app, and the local ZKit package.
    let remote = Source::Remote { url: url.clone(), rule: Rule::UpToNextMajor("1.0.0".into()) };
    let (text, tally) = add_package(&proj, &original, &remote, &[("Tally".into(), APP.into()), ("TallyFormat".into(), APP.into())]).unwrap();
    let local = Source::Local { path: work.join("ZKit") };
    let (text, zkit) = add_package(&proj, &text.unwrap(), &local, &[("ZKit".into(), APP.into())]).unwrap();
    let text = text.unwrap();
    zcode::xcodeproj::write_project_file(&pbxproj, &text).unwrap();
    std::fs::write(
        work.join("HelloClassic/HelloClassic/UsesPackages.swift"),
        "import Tally\nimport TallyFormat\nimport ZKit\n\nfunc packageCheck() -> String {\n    var tally = Tally()\n    tally.add(3)\n    return marks(tally) + Tally.version + ZKit.Greeter(name: \"z\").greeting()\n}\n",
    )
    .unwrap();
    // Group-based: the new file needs a reference too.
    let file = work.join("HelloClassic/HelloClassic/UsesPackages.swift");
    let text = zcode::xcodeproj::files::add_files(&proj, &text, &[file], &[APP.into()], &Default::default()).unwrap().unwrap();
    zcode::xcodeproj::write_project_file(&pbxproj, &text).unwrap();

    let derived = work.join("DerivedData");
    let (ok, log) = xcodebuild(&proj, &derived, &["-resolvePackageDependencies"]);
    assert!(ok, "resolve failed:\n{log}");
    let project = ProjectInfo { root: work.join("HelloClassic"), name: "HelloClassic".into(), kind: ProjectKind::XcodeProject(proj.clone()) };
    let resolved_file = packages::location::resolved_file(&project.kind, &project.root).unwrap();
    let pins = resolved::parse(&std::fs::read_to_string(&resolved_file).unwrap()).unwrap();
    assert_eq!(pins.len(), 1, "{pins:?}");
    assert_eq!((pins[0].identity.as_str(), pins[0].version.as_deref()), ("tally", Some("1.2.0")));
    assert!(derived.join("SourcePackages/checkouts/Tally/Sources/Tally/Tally.swift").exists());
    let checkouts = packages::location::checkouts(&derived.join("SourcePackages"));
    assert!(checkouts["tally"].ends_with("checkouts/Tally"), "{checkouts:?}");

    let (ok, log) = xcodebuild(&proj, &derived, &["-scheme", "HelloClassic", "-destination", "platform=macOS", "-derivedDataPath", derived.to_str().unwrap(), "build", "CODE_SIGNING_ALLOWED=NO"]);
    assert!(ok, "build failed:\n{}", &log[log.len().saturating_sub(4000)..]);

    // A new release: Update Package moves to it.
    let clone = work.join("tally-clone");
    let git = |args: &[&str], dir: &Path| {
        let ok = Command::new("/usr/bin/git").args(["-c", "user.name=t", "-c", "user.email=t@t.invalid"]).args(args).current_dir(dir).status().unwrap().success();
        assert!(ok, "git {args:?}");
    };
    git(&["clone", "-q", repo.to_str().unwrap(), clone.to_str().unwrap()], &work);
    git(&["commit", "-q", "--allow-empty", "-m", "1.3.0"], &clone);
    git(&["tag", "1.3.0"], &clone);
    git(&["push", "-q", "origin", "main", "1.3.0"], &clone);
    let (ok, log) = xcodebuild(&proj, &derived, &["-resolvePackageDependencies"]);
    assert!(ok, "{log}");
    let version = || resolved::parse(&std::fs::read_to_string(&resolved_file).unwrap()).unwrap()[0].version.clone();
    assert_eq!(version().as_deref(), Some("1.2.0"), "resolving alone keeps the pinned version");
    let graph = packages::Dependencies { packages_dir: Some(derived.join("SourcePackages")), ..Default::default() };
    packages::prepare(&project, &Operation::Update("tally".into()), &graph, &work).unwrap();
    let (ok, log) = xcodebuild(&proj, &derived, &["-resolvePackageDependencies"]);
    assert!(ok, "{log}");
    assert_eq!(version().as_deref(), Some("1.3.0"));

    // Remove Package takes the references, products and build files out.
    let text = std::fs::read_to_string(&pbxproj).unwrap();
    let text = remove_package(&text, &tally).unwrap().unwrap();
    let text = remove_package(&text, &zkit).unwrap().unwrap();
    assert!(ProjectPackages::parse(&text).unwrap().references.is_empty());
    assert!(!text.contains("productRef = ") && !text.contains("Tally") && !text.contains("ZKit"), "{text}");
    std::fs::remove_dir_all(&work).ok();
}

#[test]
#[ignore]
fn lists_tags_and_reads_products_offline() {
    use zcode::packages::remote;
    let work = std::env::temp_dir().join(format!("zcode-packages-remote-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let repo = tally_repo(&work.canonicalize().unwrap());
    let url = format!("file://{}", repo.display());
    let refs = remote::list_refs(&url).unwrap();
    assert_eq!(refs.releases.iter().map(|r| r.tag.as_str()).collect::<Vec<_>>(), ["2.0.0", "1.2.0", "1.1.0", "1.0.0"]);
    assert_eq!(refs.default_branch.as_deref(), Some("main"));
    assert_eq!(refs.branches, ["main", "develop"]);

    let rule = Rule::UpToNextMinor("1.1.0".into());
    let tag = remote::checkout_ref(&rule, &refs.releases).unwrap();
    assert_eq!(tag, "1.1.0");
    let manifest = remote::fetch_manifest(&url, &tag, false, &work.join("scratch")).unwrap();
    assert_eq!(manifest.name, "Tally");
    assert_eq!(manifest.products.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Tally", "TallyFormat"]);

    // By commit: the develop branch's head.
    let output = Command::new("/usr/bin/git").arg("-C").arg(&repo).args(["rev-parse", "develop"]).output().unwrap();
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let manifest = remote::fetch_manifest(&url, &sha, true, &work.join("scratch")).unwrap();
    assert_eq!(manifest.products.len(), 2);
    assert!(remote::list_refs("file:///nonexistent/Nope.git").unwrap_err().to_string().contains("does not appear to be a git repository"));
    std::fs::remove_dir_all(&work).ok();
}

/// Needs network access (github.com).
#[test]
#[ignore]
fn network_lists_a_public_packages_versions() {
    let refs = zcode::packages::remote::list_refs(&zcode::packages::remote::normalize_url("apple/swift-argument-parser")).unwrap();
    assert!(refs.releases.iter().any(|r| r.tag == "1.0.0"), "{:?}", refs.releases.first());
    assert_eq!(refs.default_branch.as_deref(), Some("main"));
}

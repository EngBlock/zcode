//! Makes the project navigator's structural edits to a copy of the
//! HelloClassic sample (new file in a new folder, rename, move, trash, a
//! resource, a test-target membership) and checks xcodebuild still builds
//! the app and its tests. Requires Xcode; `cargo test -- --ignored`.

use std::path::{Path, PathBuf};
use std::process::Command;

use zcode::xcodeproj::files::{self, AddOptions};

const APP: &str = "7C1E5A0029F0B1C200D4E8F1";
const TESTS: &str = "7C1E5A0F29F0B1C200D4E8F1";

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

fn changed(result: anyhow::Result<Option<String>>) -> String {
    result.unwrap().expect("the edit should change the project")
}

#[test]
#[ignore]
fn xcodebuild_builds_the_edited_project() {
    let work = std::env::temp_dir().join(format!("zcode-pbxproj-build-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let dir = work.join("HelloClassic");
    copy_dir(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("samples/HelloClassic"), &dir);
    let dir = dir.canonicalize().unwrap();
    let proj = dir.join("HelloClassic.xcodeproj");
    let pbxproj = proj.join("project.pbxproj");
    let mut text = std::fs::read_to_string(&pbxproj).unwrap();

    // New File in a folder the project has no group for.
    std::fs::create_dir_all(dir.join("HelloClassic/Views")).unwrap();
    std::fs::write(dir.join("HelloClassic/Views/Badge.swift"), "struct Badge { var label = \"new\" }\n").unwrap();
    let badge = dir.join("HelloClassic/Views/Badge.swift");
    let targets = files::ProjectFiles::parse(&proj, &text).unwrap().suggested_targets(&badge);
    assert_eq!(targets, [APP]);
    text = changed(files::add_files(&proj, &text, &[badge], &targets, &AddOptions::default()));

    // Rename Greeter.swift, then move ContentView.swift into Views/.
    let (greeter, greeting) = (dir.join("HelloClassic/Model/Greeter.swift"), dir.join("HelloClassic/Model/Greeting.swift"));
    std::fs::rename(&greeter, &greeting).unwrap();
    text = changed(files::move_path(&proj, &text, &greeter, &greeting));
    let (content, moved) = (dir.join("HelloClassic/ContentView.swift"), dir.join("HelloClassic/Views/ContentView.swift"));
    std::fs::rename(&content, &moved).unwrap();
    text = changed(files::move_path(&proj, &text, &content, &moved));

    // A file that doesn't compile, added and then trashed.
    let junk = dir.join("HelloClassic/Junk.swift");
    std::fs::write(&junk, "this does not compile\n").unwrap();
    text = changed(files::add_files(&proj, &text, &[junk.clone()], &[APP.into()], &AddOptions::default()));
    std::fs::remove_file(&junk).unwrap();
    text = changed(files::remove_paths(&proj, &text, &[junk]));

    // A resource, and a shared source file checked into the test target.
    std::fs::write(dir.join("HelloClassic/greeting.json"), "{\"hello\": \"zcode\"}\n").unwrap();
    text = changed(files::add_files(&proj, &text, &[dir.join("HelloClassic/greeting.json")], &[APP.into()], &AddOptions::default()));
    std::fs::write(dir.join("HelloClassicTests/BadgeTests.swift"), "import XCTest\n@testable import HelloClassic\n\nfinal class BadgeTests: XCTestCase {\n    func testLabel() { XCTAssertEqual(Badge().label, \"new\") }\n}\n").unwrap();
    text = changed(files::add_files(&proj, &text, &[dir.join("HelloClassicTests/BadgeTests.swift")], &[TESTS.into()], &AddOptions::default()));
    text = changed(files::set_target_membership(&proj, &text, &dir.join("HelloClassic/Views/Badge.swift"), TESTS, true));
    text = changed(files::set_target_membership(&proj, &text, &dir.join("HelloClassic/Views/Badge.swift"), TESTS, false));

    zcode::xcodeproj::write_project_file(&pbxproj, &text).unwrap();
    let derived = work.join("DerivedData");
    let output = Command::new("/usr/bin/xcrun")
        .args(["xcodebuild", "-project"])
        .arg(&proj)
        .args(["-scheme", "HelloClassic", "-derivedDataPath"])
        .arg(&derived)
        .arg("build-for-testing")
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "xcodebuild failed:\n{}\n{}", &log[log.len().saturating_sub(4000)..], String::from_utf8_lossy(&output.stderr));
    for compiled in ["Badge.swift", "Greeting.swift", "Views/ContentView.swift", "BadgeTests.swift"] {
        assert!(log.contains(compiled), "{compiled} wasn't compiled");
    }
    assert!(!log.contains("Junk.swift"));
    let app = derived.join("Build/Products/Debug/HelloClassic.app");
    assert!(app.join("Contents/Resources/greeting.json").exists(), "the resource wasn't copied");
    let _ = std::fs::remove_dir_all(&work);
}

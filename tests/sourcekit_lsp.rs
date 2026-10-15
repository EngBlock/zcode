//! End-to-end check that sourcekit-lsp, fed the compilation database zcode
//! extracts from an xcodebuild log, resolves cross-file definitions in an
//! `.xcodeproj` target. Requires Xcode; run with `cargo test -- --ignored`.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde_json::json;
use zcode::build::compiledb;
use zcode::lsp::{LanguageServer, path_to_uri};

fn sample_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../samples/HelloZ")
}

#[test]
#[ignore]
fn definition_across_files_in_xcode_project() {
    let root = sample_root().canonicalize().expect("sample project exists");
    let derived = std::env::temp_dir().join("zcode-it-dd");
    let output = Command::new("/usr/bin/xcodebuild")
        .current_dir(&root)
        .args(["build", "-project", "HelloZ.xcodeproj", "-scheme", "HelloZ"])
        .args(["-destination", "generic/platform=iOS Simulator"])
        .arg("-derivedDataPath")
        .arg(&derived)
        .output()
        .expect("xcodebuild runs");
    let log = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(output.status.success(), "build failed:\n{log}");

    let commands = compiledb::extract(&log);
    assert!(
        commands.iter().any(|c| c.file.ends_with("Counter.swift")),
        "compile db has Counter.swift (incremental builds may skip SwiftDriver; clean {derived:?})"
    );
    let cache = std::env::temp_dir().join("zcode-it-cache");
    std::fs::create_dir_all(&cache).unwrap();
    let db = compiledb::database_path(&cache);
    std::fs::remove_file(&db).ok();
    compiledb::merge_into(&db, commands).unwrap();

    let rel = zcode::paths::relative_path(&root, &cache);
    let binary = zcode::toolchain::find_tool("sourcekit-lsp").unwrap();
    let (tx, _rx) = smol::channel::unbounded();
    let server = LanguageServer::spawn(
        "sourcekit-lsp",
        &binary,
        &["--compilation-db-search-path".into(), rel.display().to_string()],
        &root,
        None,
        tx,
    )
    .unwrap();

    smol::block_on(async {
        server.initialize(&root, json!({})).await.unwrap();
        let file = root.join("HelloZ/ContentView.swift");
        let text = std::fs::read_to_string(&file).unwrap();
        server.notify_raw(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": path_to_uri(&file).as_str(), "languageId": "swift", "version": 1, "text": text,
            }}),
        );
        // `Counter()` on line 4 (0-based 3).
        let line = text.lines().position(|l| l.contains("Counter()")).unwrap();
        let character = text.lines().nth(line).unwrap().find("Counter()").unwrap() + 2;
        let mut found = None;
        for _ in 0..30 {
            let result = server
                .request_raw(
                    "textDocument/definition",
                    json!({
                        "textDocument": { "uri": path_to_uri(&file).as_str() },
                        "position": { "line": line, "character": character },
                    }),
                )
                .await
                .unwrap();
            if result.to_string().contains("Counter.swift") {
                found = Some(result);
                break;
            }
            smol::Timer::after(Duration::from_millis(500)).await;
        }
        assert!(found.is_some(), "definition of Counter resolves to Counter.swift");
        server.shutdown().await;
    });
}

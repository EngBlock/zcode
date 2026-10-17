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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("samples/HelloZ")
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

/// Inlay hints, semantic tokens, signature help and the call and type
/// hierarchies, from the installed sourcekit-lsp, through zcode's
/// conversions, on a small Swift package written for the test.
#[test]
#[ignore]
fn language_features_on_a_swift_package() {
    use zcode::lsp::hierarchy::{self, HierarchyKind};
    use zcode::lsp::{Features, inlay_hints, offset_to_lsp, semantic_tokens};
    use zcode_editor::Rope;

    let root = std::env::temp_dir().join(format!("zcode-it-features-{}", std::process::id()));
    std::fs::create_dir_all(root.join("Sources/Kit")).unwrap();
    let root = root.canonicalize().unwrap();
    std::fs::write(
        root.join("Package.swift"),
        "// swift-tools-version:5.9\nimport PackageDescription\nlet package = Package(name: \"Kit\", targets: [.target(name: \"Kit\")])\n",
    )
    .unwrap();
    let source = "protocol Shape { func area() -> Double }\n\
                  class Base: Shape { func area() -> Double { 0 } }\n\
                  final class Square: Base {\n    let side: Double\n    init(side: Double) { self.side = side }\n}\n\
                  func total(of shapes: [Shape]) -> Double {\n    let areas = shapes.map { $0.area() }\n    return areas.reduce(0, +)\n}\n\
                  func report() {\n    let sq = Square(side: 2)\n    print(total(of: [sq]))\n}\n";
    let file = root.join("Sources/Kit/Shapes.swift");
    std::fs::write(&file, source).unwrap();
    let text = Rope::from(source);
    let at = |needle: &str, offset: usize| offset_to_lsp(&text, source.find(needle).unwrap() + offset);

    let binary = zcode::toolchain::find_tool("sourcekit-lsp").unwrap();
    let (tx, _rx) = smol::channel::unbounded();
    let server = LanguageServer::spawn("sourcekit-lsp", &binary, &[], &root, None, tx).unwrap();
    let uri = path_to_uri(&file);
    let document = json!({ "uri": uri.as_str() });

    smol::block_on(async {
        server.initialize(&root, json!({ "backgroundIndexing": true })).await.unwrap();
        let features = Features::from_capabilities(&server.capabilities());
        assert!(features.inlay_hints && features.call_hierarchy && features.type_hierarchy, "{features:?}");
        let legend = features.semantic_tokens.clone().expect("semantic tokens");
        server.notify_raw(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri.as_str(), "languageId": "swift", "version": 1, "text": source } }),
        );

        // Hints come once the file's build settings are known.
        let mut hints = vec![];
        for _ in 0..60 {
            let range = inlay_hints::request_range(&text, 0..20);
            let value = server.request_raw("textDocument/inlayHint", json!({ "textDocument": document, "range": range })).await.unwrap();
            hints = inlay_hints::to_hints(&text, serde_json::from_value(value).unwrap_or_default());
            if !hints.is_empty() {
                break;
            }
            smol::Timer::after(Duration::from_millis(500)).await;
        }
        let after_areas = source.find("let areas").unwrap() + "let areas".len();
        assert!(hints.iter().any(|h| h.offset == after_areas && h.text == ": [Double]" && !h.before_text), "{hints:?}");

        let value = server.request_raw("textDocument/semanticTokens/full", json!({ "textDocument": document })).await.unwrap();
        let data: Vec<u32> = serde_json::from_value(value["data"].clone()).unwrap();
        let tokens = semantic_tokens::decode(&text, &data, &legend);
        let square_call = source.find("Square(side: 2)").unwrap();
        assert!(tokens.contains(&(square_call..square_call + "Square".len(), "type")), "{tokens:?}");

        let value = server
            .request_raw("textDocument/signatureHelp", json!({ "textDocument": document, "position": at("total(of: [sq])", 6) }))
            .await
            .unwrap();
        let help: lsp_types::SignatureHelp = serde_json::from_value(value).unwrap();
        let (_, parameter, _) = zcode_editor::split_signature_label(&help.signatures[0], help.active_parameter);
        assert_eq!(parameter, "of: [any Shape]");

        // The hierarchies come from the index, built in the background.
        let mut callers = vec![];
        for _ in 0..240 {
            let roots = hierarchy::prepare(server.clone(), HierarchyKind::Callers, file.clone(), at("func total", 6)).await.unwrap();
            if let Some(root) = roots.first() {
                callers = hierarchy::children(server.clone(), HierarchyKind::Callers, root).await.unwrap();
                if !callers.is_empty() {
                    let callees = hierarchy::children(server.clone(), HierarchyKind::Callees, root).await.unwrap();
                    assert!(callees.iter().any(|c| c.name == "Shape.area()"), "{callees:?}");
                    break;
                }
            }
            smol::Timer::after(Duration::from_millis(500)).await;
        }
        assert_eq!(callers.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["report()"]);
        assert_eq!(callers[0].call_sites.len(), 1);

        let base = hierarchy::prepare(server.clone(), HierarchyKind::Subtypes, file.clone(), at("class Base", 7)).await.unwrap();
        let base = base.first().expect("Base");
        let names = |items: Vec<hierarchy::HierarchyItem>| items.into_iter().map(|i| i.name).collect::<Vec<_>>();
        assert_eq!(names(hierarchy::children(server.clone(), HierarchyKind::Supertypes, base).await.unwrap()), ["Shape"]);
        assert_eq!(names(hierarchy::children(server.clone(), HierarchyKind::Subtypes, base).await.unwrap()), ["Square"]);
        server.shutdown().await;
    });
    std::fs::remove_dir_all(&root).ok();
}

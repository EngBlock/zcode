//! Extracts a JSON compilation database from xcodebuild output so
//! sourcekit-lsp understands `.xcodeproj` targets (module names, SDK,
//! search paths, cross-file references) — the job xcode-build-server does.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompileCommand {
    pub directory: String,
    pub file: String,
    pub arguments: Vec<String>,
}

/// Flags that only matter for producing build outputs, or that make
/// sourcekitd try to use Xcode's explicitly-built module graph.
const DROP_FLAGS: &[&str] = &[
    "-c",
    "-incremental",
    "-enable-batch-mode",
    "-explicit-module-build",
    "-experimental-emit-module-separately",
    "-emit-dependencies",
    "-emit-module",
    "-emit-objc-header",
    "-emit-const-values",
    "-emit-localized-strings",
    "-serialize-diagnostics",
    "-save-temps",
    "-no-color-diagnostics",
    "-validate-clang-modules-once",
    "-profile-generate",
    "-profile-coverage-mapping",
    "-parseable-output",
    "-use-frontend-parseable-output",
    "-disable-cmo",
];

/// Flags followed by a value that should both be dropped.
const DROP_WITH_VALUE: &[&str] = &[
    "-o",
    "-output-file-map",
    "-emit-module-path",
    "-emit-objc-header-path",
    "-emit-localized-strings-path",
    "-const-gather-protocols-list",
    "-dependency-scan-serialize-diagnostics-path",
    "-clang-scanner-module-cache-path",
    "-clang-build-session-file",
    "-ivfsstatcache",
    "-MF",
    "--serialize-diagnostics",
];

fn clean_swift_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if DROP_FLAGS.contains(&arg.as_str()) || arg.starts_with("-j") && arg.len() > 2 {
            i += 1;
            continue;
        }
        if DROP_WITH_VALUE.contains(&arg.as_str()) {
            i += 2;
            continue;
        }
        // `-Xcc -ivfsstatcache -Xcc <path>` and similar paired clang flags.
        if arg == "-Xcc"
            && let Some(next) = args.get(i + 1)
            && (next == "-ivfsstatcache" || next == "-fbuild-session-file")
        {
            i += 4;
            continue;
        }
        if arg == "-Xcc" && args.get(i + 1).is_some_and(|n| n.starts_with("-fbuild-session")) {
            i += 2;
            continue;
        }
        out.push(arg.clone());
        i += 1;
    }
    out
}

/// Expands `@file` response files (one path per line) into arguments.
fn expand_response_files(args: Vec<String>) -> (Vec<String>, Vec<String>) {
    let mut out = Vec::with_capacity(args.len());
    let mut sources = Vec::new();
    for arg in args {
        if let Some(path) = arg.strip_prefix('@')
            && let Ok(contents) = std::fs::read_to_string(path)
        {
            for line in contents.lines() {
                let file = line.trim().trim_matches('"').replace("\\ ", " ");
                if !file.is_empty() {
                    sources.push(file.clone());
                    out.push(file);
                }
            }
            continue;
        }
        out.push(arg);
    }
    (out, sources)
}

/// Scans a complete xcodebuild log for compiler invocations.
pub fn extract(log: &str) -> Vec<CompileCommand> {
    let mut commands = Vec::new();
    let lines: Vec<&str> = log.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let is_swift = line.starts_with("SwiftDriver ");
        let is_clang = line.starts_with("CompileC ");
        if !(is_swift || is_clang) {
            i += 1;
            continue;
        }
        // The task header is followed by indented "cd" and command lines.
        let mut directory = String::new();
        let mut j = i + 1;
        while j < lines.len() && lines[j].starts_with("    ") {
            let body = lines[j].trim_start();
            if let Some(dir) = body.strip_prefix("cd ") {
                directory = dir.trim().to_string();
            } else if is_swift && let Some(cmd) = body.split_once("builtin-SwiftDriver -- ") {
                if let Some(args) = shlex::split(cmd.1) {
                    let (args, sources) = expand_response_files(args);
                    let args = clean_swift_args(&args);
                    for source in sources {
                        commands.push(CompileCommand {
                            directory: directory.clone(),
                            file: source,
                            arguments: args.clone(),
                        });
                    }
                }
            } else if is_clang
                && (body.contains("/clang ") || body.starts_with("clang "))
                && let Some(args) = shlex::split(body)
            {
                let file = args
                    .iter()
                    .position(|a| a == "-c")
                    .and_then(|ix| args.get(ix + 1))
                    .cloned();
                if let Some(file) = file {
                    commands.push(CompileCommand {
                        directory: directory.clone(),
                        file,
                        arguments: clean_swift_args(&args),
                    });
                }
            }
            j += 1;
        }
        i = j;
    }
    commands
}

/// Merges new commands into the database at `path` (keyed by file), so
/// incremental builds that only recompile some targets keep the rest.
/// Returns whether the file changed.
/// Points every command at one index store. Builds into different
/// derived-data folders would otherwise leave sourcekit-lsp reading a stale
/// store while another one is being updated.
pub fn with_index_store(mut commands: Vec<CompileCommand>, store: &Path) -> Vec<CompileCommand> {
    let store = store.display().to_string();
    for command in &mut commands {
        let args = &mut command.arguments;
        match args.iter().position(|a| a == "-index-store-path") {
            Some(ix) if ix + 1 < args.len() => args[ix + 1] = store.clone(),
            _ => args.extend(["-index-store-path".to_string(), store.clone()]),
        }
    }
    commands
}

/// Adds an entry under each file's resolved path. xcodebuild reports paths as
/// it was given them (e.g. `/tmp/...`), while editors open the real path
/// (`/private/tmp/...`); sourcekit-lsp matches entries by exact path, so files
/// reached through a symlink would otherwise get no build settings.
pub fn with_canonical_paths(commands: Vec<CompileCommand>) -> Vec<CompileCommand> {
    let mut out = Vec::with_capacity(commands.len());
    for command in commands {
        let canonical = std::fs::canonicalize(&command.file)
            .ok()
            .map(|p| p.display().to_string())
            .filter(|p| *p != command.file);
        if let Some(file) = canonical {
            out.push(CompileCommand { file, ..command.clone() });
        }
        out.push(command);
    }
    out
}

/// The index store maintained by zcode's background index build.
pub fn index_store_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("IndexDerivedData/Index.noindex/DataStore")
}

pub fn merge_into(path: &Path, commands: Vec<CompileCommand>) -> Result<bool> {
    if commands.is_empty() {
        return Ok(false);
    }
    let mut by_file: BTreeMap<String, CompileCommand> = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<CompileCommand>>(&bytes).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|c| (c.file.clone(), c))
        .collect();
    let before = by_file.clone();
    for command in commands {
        by_file.insert(command.file.clone(), command);
    }
    if by_file == before {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let all: Vec<_> = by_file.into_values().collect();
    std::fs::write(path, serde_json::to_vec(&all)?)?;
    Ok(true)
}

pub fn database_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("compile_commands.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_resolved_paths() {
        let dir = std::env::temp_dir().join(format!("zcode-canon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("A.swift");
        std::fs::write(&real, "").unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&dir, &link).ok();
        let via_link = link.join("A.swift").display().to_string();
        let out = with_canonical_paths(vec![CompileCommand {
            directory: "/".into(),
            file: via_link.clone(),
            arguments: vec![],
        }]);
        let files: Vec<_> = out.iter().map(|c| c.file.clone()).collect();
        assert!(files.contains(&via_link));
        assert!(files.contains(&real.canonicalize().unwrap().display().to_string()));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn normalizes_index_store() {
        let commands = vec![CompileCommand {
            directory: "/p".into(),
            file: "/p/A.swift".into(),
            arguments: vec!["swiftc".into(), "-index-store-path".into(), "/old".into()],
        }];
        let out = with_index_store(commands, Path::new("/new"));
        assert_eq!(out[0].arguments, vec!["swiftc", "-index-store-path", "/new"]);
    }

    #[test]
    fn extracts_swift_driver_commands() {
        let dir = std::env::temp_dir().join(format!("zcode-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let list = dir.join("App.SwiftFileList");
        std::fs::write(&list, "/p/App/A.swift\n/p/App/B\\ C.swift\n").unwrap();
        let log = format!(
            "SwiftDriver App normal arm64 com.apple.xcode.tools.swift.compiler (in target 'App' from project 'App')\n    cd /p\n    builtin-SwiftDriver -- /usr/bin/swiftc -module-name App -Onone @{} -sdk /SDK -target arm64-apple-ios18.0-simulator -c -j16 -output-file-map /tmp/map.json -explicit-module-build -Xcc -ivfsstatcache -Xcc /tmp/x -enforce-exclusivity\\=checked\n\nSwiftCompile normal arm64 x\n",
            list.display()
        );
        let commands = extract(&log);
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].file, "/p/App/A.swift");
        assert_eq!(commands[1].file, "/p/App/B C.swift");
        let args = &commands[0].arguments;
        assert!(args.contains(&"-module-name".to_string()));
        assert!(args.contains(&"/p/App/B C.swift".to_string()));
        assert!(args.contains(&"-enforce-exclusivity=checked".to_string()));
        for dropped in ["-c", "-j16", "-output-file-map", "-explicit-module-build", "-ivfsstatcache"] {
            assert!(!args.contains(&dropped.to_string()), "{dropped} should be dropped");
        }
        std::fs::remove_dir_all(dir).ok();
    }
}

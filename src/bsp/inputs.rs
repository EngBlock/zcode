//! What a build graph is made from, read off the main thread: the project
//! model's targets and what Xcode says about how to compile them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

use super::graph::{IndexArguments, Kind, ModelTarget};
use crate::build::xcode::Destination;
use crate::project::ProjectInfo;
use crate::xcodeproj::XcodeProject;
use crate::xcodeproj::files::ProjectFiles;

/// The native targets of each project, with what they compile. Reads
/// `project.pbxproj` and the synchronized folders.
pub fn model_targets(projects: &[PathBuf]) -> Vec<ModelTarget> {
    let mut out = vec![];
    for project in projects {
        let text = match std::fs::read_to_string(XcodeProject::pbxproj_path(project)) {
            Ok(text) => text,
            Err(err) => {
                log::warn!("build server: couldn’t read {}: {err}", project.display());
                continue;
            }
        };
        let files = match ProjectFiles::parse(project, &text) {
            Ok(files) => files,
            Err(err) => {
                log::warn!("build server: couldn’t parse {}: {err:#}", project.display());
                continue;
            }
        };
        let name = project.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        for target in files.targets.iter().filter(|t| t.isa == "PBXNativeTarget") {
            out.push(ModelTarget {
                name: target.name.clone(),
                project: name.clone(),
                kind: Some(Kind::from_product_type(target.product_type.as_deref())),
                files: files.compiled_files(target),
                directory: files.project_dir.clone(),
            });
        }
    }
    out
}

/// The scheme, configuration and destination Xcode is asked about: the
/// index build's.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub scheme: String,
    pub configuration: String,
    pub destination: Destination,
}

/// What the queries returned.
#[derive(Clone, Debug, Default)]
pub struct Queried {
    pub index_settings: Arc<HashMap<String, HashMap<PathBuf, IndexArguments>>>,
    pub build_settings: Arc<HashMap<String, HashMap<String, String>>>,
    /// What went wrong, for `bsp status`.
    pub errors: Vec<String>,
}

fn xcodebuild(project: &ProjectInfo, query: &Query, derived_data: &Path, what: &str, action: Option<&str>) -> Command {
    let mut cmd = Command::new("/usr/bin/xcodebuild");
    cmd.current_dir(&project.root)
        .arg(what)
        .arg("-json")
        .args(project.kind.xcodebuild_args())
        .args(["-scheme", &query.scheme, "-configuration", &query.configuration])
        .args(["-destination", &query.destination.xcodebuild_specifier()])
        .arg("-derivedDataPath")
        .arg(derived_data)
        // The index build resolves packages; don't fetch them again.
        .args(["-skipPackageUpdates", "-skipMacroValidation", "-skipPackagePluginValidation"]);
    cmd.args(action);
    crate::build::process::unblock_signals(&mut cmd);
    cmd
}

fn run_json(mut cmd: Command) -> Result<Value> {
    let output = cmd.stdin(std::process::Stdio::null()).output().context("running xcodebuild")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{}", stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("xcodebuild failed").trim());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let start = stdout.find(['{', '[']).unwrap_or(0);
    Ok(serde_json::from_str(&stdout[start..])?)
}

/// Runs a query for the scheme's test targets too (`build-for-testing`),
/// or, for a scheme that can't test, its build alone.
fn run_query(project: &ProjectInfo, query: &Query, derived_data: &Path, what: &str) -> Result<Value> {
    run_json(xcodebuild(project, query, derived_data, what, Some("build-for-testing")))
        .or_else(|_| run_json(xcodebuild(project, query, derived_data, what, None)))
}

/// Asks Xcode for every file's arguments for indexing, and every target's
/// build settings. Blocking (seconds on a big app).
pub fn query(project: &ProjectInfo, query: &Query, derived_data: &Path) -> Queried {
    let mut queried = Queried::default();
    let started = std::time::Instant::now();
    match run_query(project, query, derived_data, "-showBuildSettingsForIndex") {
        Ok(value) => queried.index_settings = Arc::new(parse_index_settings(&value)),
        Err(err) => queried.errors.push(format!("-showBuildSettingsForIndex: {err:#}")),
    }
    match run_query(project, query, derived_data, "-showBuildSettings") {
        Ok(value) => queried.build_settings = Arc::new(parse_build_settings(&value)),
        Err(err) => queried.errors.push(format!("-showBuildSettings: {err:#}")),
    }
    let with_arguments = queried.index_settings.values().flat_map(|files| files.values()).filter(|a| a.swift.is_some() || a.clang.is_some()).count();
    log::info!(
        "build server: asked xcodebuild about {} on {} in {:?}: index arguments for {with_arguments} files, settings for {} targets{}",
        query.scheme,
        query.destination.name,
        started.elapsed(),
        queried.build_settings.len(),
        if queried.errors.is_empty() { String::new() } else { format!(" ({})", queried.errors.join("; ")) },
    );
    queried
}

/// `-showBuildSettingsForIndex -json`: target → file → arguments. Files
/// without arguments (Xcode lists a target's files even when it couldn't
/// plan its build) are left out.
pub fn parse_index_settings(value: &Value) -> HashMap<String, HashMap<PathBuf, IndexArguments>> {
    let strings = |value: Option<&Value>| -> Option<Vec<String>> {
        Some(value?.as_array()?.iter().filter_map(Value::as_str).map(str::to_string).collect()).filter(|v: &Vec<String>| !v.is_empty())
    };
    let mut out = HashMap::new();
    for (target, files) in value.as_object().into_iter().flatten() {
        let files: HashMap<PathBuf, IndexArguments> = files
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(file, entry)| {
                let arguments =
                    IndexArguments { swift: strings(entry.get("swiftASTCommandArguments")), clang: strings(entry.get("clangASTCommandArguments")) };
                (arguments.swift.is_some() || arguments.clang.is_some()).then(|| (PathBuf::from(file), arguments))
            })
            .collect();
        if !files.is_empty() {
            out.insert(target.clone(), files);
        }
    }
    out
}

/// `-showBuildSettings -json`: target → setting → value.
pub fn parse_build_settings(value: &Value) -> HashMap<String, HashMap<String, String>> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let target = entry.get("target")?.as_str()?.to_string();
            let settings = entry.get("buildSettings")?.as_object()?.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect();
            Some((target, settings))
        })
        .collect()
}

/// What, changing on disk, changes the graph: the projects' files, the
/// compilation database, and the folders synchronized folders hold (a
/// file added to or removed from one changes its folder's time).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stamps {
    pub projects: Vec<Option<SystemTime>>,
    pub database: Option<SystemTime>,
    pub folders: Vec<(PathBuf, Option<SystemTime>)>,
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Stamps {
    /// Reads them. `synchronized` are the synchronized folders' roots.
    pub fn read(projects: &[PathBuf], database: &Path, synchronized: &[PathBuf]) -> Self {
        let mut folders = vec![];
        let mut stack: Vec<PathBuf> = synchronized.to_vec();
        while let Some(dir) = stack.pop() {
            folders.push((dir.clone(), modified(&dir)));
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir()) && !entry.file_name().to_string_lossy().starts_with('.') {
                    stack.push(entry.path());
                }
            }
        }
        folders.sort();
        Self {
            projects: projects.iter().map(|p| modified(&XcodeProject::pbxproj_path(p))).collect(),
            database: modified(database),
            folders,
        }
    }
}

/// The synchronized folders of `projects`.
pub fn synchronized_folders(projects: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = vec![];
    for project in projects {
        let Ok(text) = std::fs::read_to_string(XcodeProject::pbxproj_path(project)) else { continue };
        let Ok(files) = ProjectFiles::parse(project, &text) else { continue };
        out.extend(
            files.items.values().filter(|i| i.isa == crate::xcodeproj::files::SYNCHRONIZED_GROUP).filter_map(|i| i.resolved.clone()),
        );
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_index_arguments_and_settings() {
        let value = json!({
            "App": {
                "/p/App/A.swift": { "LanguageDialect": "Xcode.SourceCodeLanguage.Swift", "swiftASTCommandArguments": ["-module-name", "App", "/p/App/A.swift"] },
                "/p/App/b.m": { "clangASTCommandArguments": ["-x", "objective-c", "/p/App/b.m"] },
            },
            // Xcode couldn't plan this one's build: no arguments.
            "Client": { "/p/Client/C.swift": { "LanguageDialect": "Xcode.SourceCodeLanguage.Swift" } },
        });
        let index = parse_index_settings(&value);
        assert_eq!(index.len(), 1);
        let app = &index["App"];
        assert_eq!(app[Path::new("/p/App/A.swift")].swift.as_ref().unwrap()[1], "App");
        assert!(app[Path::new("/p/App/b.m")].clang.is_some() && app[Path::new("/p/App/b.m")].swift.is_none());

        let settings = parse_build_settings(&json!([
            { "target": "App", "buildSettings": { "PRODUCT_MODULE_NAME": "App", "SDKROOT": "/SDK" } },
            { "target": "AppTests", "buildSettings": { "PRODUCT_MODULE_NAME": "AppTests" } },
        ]));
        assert_eq!(settings["App"]["SDKROOT"], "/SDK");
        assert_eq!(settings["AppTests"]["PRODUCT_MODULE_NAME"], "AppTests");
    }

    #[test]
    fn a_file_added_to_a_synchronized_folder_changes_the_stamps() {
        let dir = std::env::temp_dir().join(format!("zcode-bsp-stamps-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Nested")).unwrap();
        let before = Stamps::read(&[], &dir.join("none.json"), std::slice::from_ref(&dir));
        assert_eq!(before.folders.len(), 2);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("Nested/New.swift"), "").unwrap();
        let after = Stamps::read(&[], &dir.join("none.json"), std::slice::from_ref(&dir));
        assert_ne!(before, after);
        std::fs::remove_dir_all(&dir).ok();
    }
}

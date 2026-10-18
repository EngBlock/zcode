//! The build graph the build server answers from: a project's targets, the
//! files each compiles, and how to compile them.
//!
//! Membership comes from the project model (`project.pbxproj`'s build
//! phases and synchronized folders), so a new file, or one added to or
//! taken out of a target, counts at once. Compiler arguments come from the
//! best of three places, per target:
//!
//! 1. Xcode's own arguments for indexing (`xcodebuild
//!    -showBuildSettingsForIndex`): every file of every target in the
//!    scheme, built or not, following build-setting edits.
//! 2. The index build's log (the compilation database, `compiledb`): what
//!    the last build compiled each target with.
//! 3. The target's build settings (`xcodebuild -showBuildSettings`), turned
//!    into a command line, with the modules the index build produced in
//!    reach: for a target neither of the others has (an app whose build
//!    stops before compiling it).
//!
//! A Swift file's arguments are its target's (without the file list) and
//! the target's current Swift files; a C-family file has its own, or a
//! sibling's of the same language with the file swapped in. A file of a
//! target nothing says how to compile yet gets the file alone with the
//! default SDK, as sourcekit-lsp would without a build server: answering
//! nothing would leave it on sourcekit-lsp's fallback, which has no
//! semantic tokens behind a build server.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::build::compiledb::CompileCommand;

/// Where a target's arguments came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    IndexSettings,
    BuildLog,
    BuildSettings,
    /// The file alone, until something better is known.
    FileAlone,
}

/// What a target is, as BSP tags it. Its order is the order targets sort
/// in, and sourcekit-lsp uses a file's first target: an app's file shared
/// with its tests is the app's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Application,
    Library,
    Test,
    /// A package's target, known only from the build log.
    Dependency,
}

impl Kind {
    pub fn from_product_type(product_type: Option<&str>) -> Self {
        match product_type.unwrap_or("") {
            t if t.contains("bundle.unit-test") || t.contains("bundle.ui-testing") => Self::Test,
            t if t.contains("application") || t.contains("app-extension") || t.contains("tool") => Self::Application,
            _ => Self::Library,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Library => "library",
            Self::Test => "test",
            Self::Dependency => "dependency",
        }
    }
}

/// A target of the project model.
#[derive(Clone, Debug, Default)]
pub struct ModelTarget {
    pub name: String,
    /// Its `.xcodeproj`'s name, without the extension.
    pub project: String,
    pub kind: Option<Kind>,
    /// What it compiles (sources and headers), resolved.
    pub files: Vec<PathBuf>,
    /// Its project's folder: the compiler's working directory.
    pub directory: PathBuf,
    /// The folders of the headers its project references, which Xcode's
    /// header maps make reachable by name (a bridging header's imports).
    pub header_dirs: Vec<PathBuf>,
}

/// Xcode's arguments for indexing one file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IndexArguments {
    pub swift: Option<Vec<String>>,
    pub clang: Option<Vec<String>>,
}

/// Everything a graph is made from.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    pub root: PathBuf,
    pub model: Vec<ModelTarget>,
    /// The compilation database.
    pub commands: Vec<CompileCommand>,
    /// By target name, then file.
    pub index_settings: Arc<HashMap<String, HashMap<PathBuf, IndexArguments>>>,
    /// By target name.
    pub build_settings: Arc<HashMap<String, HashMap<String, String>>>,
    /// The index build's index store, which every command writes to and
    /// sourcekit-lsp reads.
    pub index_store: PathBuf,
    pub index_database: PathBuf,
    /// The SDK a file is compiled against when nothing else is known (the
    /// Mac's, as sourcekit-lsp's fallback).
    pub default_sdk: Option<PathBuf>,
}

/// A C-family command line for one file, which other files of the same
/// language can borrow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClangCommand {
    pub file: PathBuf,
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Target {
    /// Its BSP identifier (a URI).
    pub id: String,
    pub name: String,
    pub project: Option<String>,
    pub kind: Kind,
    pub directory: PathBuf,
    /// Sources and headers, from the project model (or the log, for a
    /// dependency).
    pub files: Vec<PathBuf>,
    /// Swift files the build generates (asset symbols and the like).
    pub generated: Vec<PathBuf>,
    /// Swift arguments without the files.
    pub swift: Option<Vec<String>>,
    pub swift_origin: Option<Origin>,
    /// C-family arguments of the files that have their own.
    pub clang: BTreeMap<PathBuf, ClangCommand>,
    /// Arguments a C-family file without its own borrows.
    pub clang_template: Option<ClangCommand>,
    pub clang_origin: Option<Origin>,
}

/// The arguments for one file.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub arguments: Vec<String>,
    pub working_directory: PathBuf,
    pub origin: Origin,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BuildGraph {
    pub targets: Vec<Target>,
    pub index_store: PathBuf,
    pub index_database: PathBuf,
    #[serde(default)]
    pub default_sdk: Option<PathBuf>,
}

/// The LSP language of a file sourcekit-lsp serves, by its extension.
pub fn language_of(path: &Path) -> Option<&'static str> {
    Some(match path.extension()?.to_str()? {
        "swift" => "swift",
        "c" => "c",
        "m" => "objective-c",
        "mm" => "objective-cpp",
        "cpp" | "cc" | "cxx" => "cpp",
        "h" | "hpp" | "hh" => return None,
        _ => return None,
    })
}

pub fn is_header(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("h" | "hpp" | "hh"))
}

/// `path` with symlinks resolved (`/var` is `/private/var`): Xcode reports
/// paths as it was given them, the project model and editors use real ones.
fn real(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn is_swift(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "swift")
}

/// clang's name for a language (`-x`).
fn clang_language(path: &Path) -> Option<&'static str> {
    Some(match language_of(path)? {
        "c" => "c",
        "objective-c" => "objective-c",
        "objective-cpp" => "objective-c++",
        "cpp" => "c++",
        _ => return None,
    })
}

/// A target's identifier: its kind first, so they sort apps, libraries,
/// tests, dependencies.
fn target_id(kind: Kind, project: Option<&str>, name: &str) -> String {
    let encode = |s: &str| -> String {
        s.bytes()
            .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
            .collect()
    };
    let rank = kind as u8;
    format!("xcode://target/{rank}/{}/{}", encode(project.unwrap_or("-")), encode(name))
}

/// The module a target builds, as Xcode names it by default.
fn default_module_name(target: &str) -> String {
    let mut out: String = target.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| a == flag).and_then(|ix| args.get(ix + 1)).map(String::as_str)
}

/// The Swift files a command line compiles.
fn swift_sources(args: &[String]) -> Vec<PathBuf> {
    args.iter()
        .enumerate()
        .filter(|(ix, arg)| arg.ends_with(".swift") && !arg.starts_with('-') && (*ix == 0 || args[ix - 1] != "-primary-file"))
        .map(|(_, arg)| PathBuf::from(arg))
        .collect()
}

/// A command line without its compiler (the log's start with one) and its
/// Swift files.
fn swift_template(args: &[String]) -> Vec<String> {
    let skip_compiler = args.first().is_some_and(|a| !a.starts_with('-') && (a.ends_with("swiftc") || a.ends_with("swift-frontend")));
    let args = &args[skip_compiler as usize..];
    args.iter()
        .enumerate()
        .filter(|(ix, arg)| !(arg.ends_with(".swift") && !arg.starts_with('-') && (*ix == 0 || args[ix - 1] != "-primary-file")))
        .map(|(_, arg)| arg.clone())
        .collect()
}

/// A C-family command line without its compiler, the parts that only
/// matter for producing outputs, and anything naming a stat cache.
fn clang_arguments(args: &[String]) -> Vec<String> {
    let skip_compiler = args.first().is_some_and(|a| !a.starts_with('-') && (a.ends_with("clang") || a.ends_with("clang++")));
    let mut out = crate::build::compiledb::clean_arguments(&args[skip_compiler as usize..]);
    if let Some(ix) = out.iter().position(|a| a == "-index-unit-output-path") {
        out.drain(ix..(ix + 2).min(out.len()));
    }
    out
}

/// Points a command line at the index build's store.
fn with_index_store(mut args: Vec<String>, store: &Path) -> Vec<String> {
    let store = store.display().to_string();
    match args.iter().position(|a| a == "-index-store-path") {
        Some(ix) if ix + 1 < args.len() => args[ix + 1] = store,
        _ => args.extend(["-index-store-path".to_string(), store]),
    }
    args
}

/// The search paths and module maps the log's Swift commands use: the
/// modules the index build produced, which a target built from its
/// settings alone can import too.
fn module_context(commands: &[CompileCommand]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = vec![];
    for command in commands.iter().filter(|c| is_swift(Path::new(&c.file))) {
        let args = &command.arguments;
        for (ix, arg) in args.iter().enumerate() {
            let next = args.get(ix + 1);
            let pair = match (arg.as_str(), next) {
                ("-I" | "-F", Some(value)) => Some((arg.clone(), value.clone())),
                ("-Xcc", Some(value)) if value.starts_with("-fmodule-map-file=") => Some((arg.clone(), value.clone())),
                _ => None,
            };
            if let Some(pair) = pair
                && seen.insert(pair.clone())
            {
                out.extend([pair.0, pair.1]);
            }
        }
    }
    out
}

/// `args` with the pairs of `context` it doesn't have yet.
fn with_context(mut args: Vec<String>, context: &[String]) -> Vec<String> {
    for pair in context.chunks(2) {
        if !args.windows(2).any(|w| w == pair) {
            args.extend(pair.iter().cloned());
        }
    }
    args
}

/// A list-valued build setting's items (`"a b" c`).
fn list(settings: &HashMap<String, String>, key: &str) -> Vec<String> {
    settings
        .get(key)
        .and_then(|v| shlex::split(v))
        .unwrap_or_default()
        .into_iter()
        .filter(|item| !item.is_empty() && item != "$(inherited)")
        .collect()
}

fn setting<'a>(settings: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    settings.get(key).map(String::as_str).filter(|v| !v.is_empty())
}

/// The target triple Xcode compiles for (`arm64-apple-ios18.0-simulator`).
fn target_triple(settings: &HashMap<String, String>) -> Option<String> {
    let arch = [setting(settings, "CURRENT_ARCH"), setting(settings, "NATIVE_ARCH_ACTUAL"), setting(settings, "NATIVE_ARCH")]
        .into_iter()
        .flatten()
        .find(|a| *a != "undefined_arch")
        .map(str::to_string)
        .or_else(|| list(settings, "ARCHS").into_iter().next())?;
    let os = match setting(settings, "LLVM_TARGET_TRIPLE_OS_VERSION") {
        Some(os) => os.to_string(),
        None => {
            let prefix = setting(settings, "SWIFT_PLATFORM_TARGET_PREFIX")?;
            let version = setting(settings, setting(settings, "DEPLOYMENT_TARGET_SETTING_NAME")?)?;
            format!("{prefix}{version}")
        }
    };
    let vendor = setting(settings, "LLVM_TARGET_TRIPLE_VENDOR").unwrap_or("apple");
    let suffix = setting(settings, "LLVM_TARGET_TRIPLE_SUFFIX").unwrap_or("");
    Some(format!("{arch}-{vendor}-{os}{suffix}"))
}

/// `SWIFT_UPCOMING_FEATURE_<NAME>` settings' feature names, where they
/// aren't the setting's words run together.
const UPCOMING_FEATURES: &[(&str, &str)] = &[
    ("DISABLE_OUTWARD_ACTOR_ISOLATION", "DisableOutwardActorInference"),
    ("IMPORT_OBJC_FORWARD_DECLS", "ImportObjcForwardDeclarations"),
    ("INFER_SENDABLE_FROM_CAPTURES", "InferSendableFromCaptures"),
];

fn upcoming_feature(setting: &str) -> String {
    if let Some((_, name)) = UPCOMING_FEATURES.iter().find(|(s, _)| *s == setting) {
        return name.to_string();
    }
    setting
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map(|c| c.to_string() + &chars.as_str().to_lowercase()).unwrap_or_default()
        })
        .collect()
}

/// Search paths and definitions both Swift and clang get from settings.
fn search_paths(settings: &HashMap<String, String>, through_xcc: bool) -> Vec<String> {
    let mut out = vec![];
    let xcc = |out: &mut Vec<String>, flag: String| {
        if through_xcc {
            out.push("-Xcc".into());
        }
        out.push(flag);
    };
    for path in list(settings, "HEADER_SEARCH_PATHS").into_iter().chain(list(settings, "USER_HEADER_SEARCH_PATHS")) {
        xcc(&mut out, format!("-I{path}"));
    }
    for path in list(settings, "SYSTEM_HEADER_SEARCH_PATHS") {
        xcc(&mut out, "-isystem".into());
        xcc(&mut out, path);
    }
    for definition in list(settings, "GCC_PREPROCESSOR_DEFINITIONS") {
        xcc(&mut out, format!("-D{definition}"));
    }
    if let Some(dir) = setting(settings, "GENERATED_MODULEMAP_DIR")
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        let mut maps: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "modulemap")).collect();
        maps.sort();
        for map in maps {
            xcc(&mut out, format!("-fmodule-map-file={}", map.display()));
        }
    }
    out
}

/// A Swift command line (without files) from a target's build settings.
pub fn swift_from_settings(settings: &HashMap<String, String>, context: &[String]) -> Option<Vec<String>> {
    let module = setting(settings, "PRODUCT_MODULE_NAME").map(str::to_string).or_else(|| setting(settings, "TARGET_NAME").map(default_module_name))?;
    let sdk = setting(settings, "SDKROOT")?;
    let mut args: Vec<String> = vec!["-module-name".into(), module, "-Onone".into(), "-enforce-exclusivity=checked".into()];
    for condition in list(settings, "SWIFT_ACTIVE_COMPILATION_CONDITIONS") {
        args.push(format!("-D{condition}"));
    }
    args.extend(["-sdk".into(), sdk.to_string()]);
    if let Some(triple) = target_triple(settings) {
        args.extend(["-target".into(), triple]);
    }
    if let Some(version) = setting(settings, "SWIFT_VERSION") {
        // "5.0" is `-swift-version 5`; "4.2" stays.
        let version = version.strip_suffix(".0").unwrap_or(version);
        args.extend(["-swift-version".into(), version.to_string()]);
    }
    args.push("-g".into());
    if setting(settings, "ENABLE_TESTABILITY") == Some("YES") {
        args.push("-enable-testing".into());
    }
    if let Some(cache) = setting(settings, "MODULE_CACHE_DIR") {
        args.extend(["-module-cache-path".into(), cache.to_string()]);
    }
    for (key, value) in settings {
        if let Some(feature) = key.strip_prefix("SWIFT_UPCOMING_FEATURE_")
            && value == "YES"
        {
            args.extend(["-enable-upcoming-feature".into(), upcoming_feature(feature)]);
        }
    }
    // Features in a fixed order, so the same settings give the same line.
    let features_start = args.iter().position(|a| a == "-enable-upcoming-feature").unwrap_or(args.len());
    let mut features: Vec<(String, String)> = args[features_start..].chunks(2).map(|c| (c[0].clone(), c[1].clone())).collect();
    features.sort();
    args.truncate(features_start);
    args.extend(features.into_iter().flat_map(|(a, b)| [a, b]));
    match setting(settings, "SWIFT_STRICT_CONCURRENCY") {
        Some(level @ ("complete" | "targeted")) => args.push(format!("-strict-concurrency={level}")),
        _ => {}
    }
    if setting(settings, "SWIFT_DEFAULT_ACTOR_ISOLATION") == Some("MainActor") {
        args.extend(["-default-isolation".into(), "MainActor".into()]);
    }
    if let Some(products) = setting(settings, "BUILT_PRODUCTS_DIR") {
        args.extend(["-I".into(), products.to_string(), "-F".into(), products.to_string()]);
    }
    for path in list(settings, "FRAMEWORK_SEARCH_PATHS") {
        args.extend(["-F".into(), path]);
    }
    for path in list(settings, "SWIFT_INCLUDE_PATHS") {
        args.extend(["-I".into(), path]);
    }
    args.extend(search_paths(settings, true));
    args.extend(context.iter().cloned());
    args.extend(list(settings, "OTHER_SWIFT_FLAGS"));
    let project_dir = setting(settings, "PROJECT_DIR").map(PathBuf::from);
    if let Some(header) = setting(settings, "SWIFT_OBJC_BRIDGING_HEADER") {
        let header = match &project_dir {
            Some(dir) => dir.join(header),
            None => PathBuf::from(header),
        };
        args.extend(["-import-objc-header".into(), header.display().to_string()]);
    }
    if let Some(dir) = project_dir {
        args.extend(["-working-directory".into(), dir.display().to_string()]);
    }
    Some(args)
}

/// A C-family command line for `file` from a target's build settings.
pub fn clang_from_settings(settings: &HashMap<String, String>, file: &Path) -> Option<Vec<String>> {
    let sdk = setting(settings, "SDKROOT")?;
    let mut args: Vec<String> = vec!["-x".into(), clang_language(file)?.into()];
    if let Some(triple) = target_triple(settings) {
        args.extend(["-target".into(), triple]);
    }
    args.extend(["-isysroot".into(), sdk.to_string(), "-fmodules".into(), "-gmodules".into(), "-O0".into()]);
    if setting(settings, "CLANG_ENABLE_OBJC_ARC") == Some("YES") {
        args.push("-fobjc-arc".into());
    }
    if let Some(cache) = setting(settings, "MODULE_CACHE_DIR") {
        args.push(format!("-fmodules-cache-path={cache}"));
    }
    if let Some(products) = setting(settings, "BUILT_PRODUCTS_DIR") {
        args.extend([format!("-I{products}/include"), format!("-F{products}")]);
    }
    for path in list(settings, "FRAMEWORK_SEARCH_PATHS") {
        args.push(format!("-F{path}"));
    }
    args.extend(search_paths(settings, false));
    args.extend(list(settings, "OTHER_CFLAGS"));
    if file.extension().is_some_and(|e| e == "cpp" || e == "cc" || e == "cxx" || e == "mm") {
        args.extend(list(settings, "OTHER_CPLUSPLUSFLAGS").into_iter().filter(|f| f != "$(OTHER_CFLAGS)"));
    }
    args.extend(["-fsyntax-only".into(), file.display().to_string()]);
    Some(args)
}

/// Borrows `command` (another file's) for `file`: the file swapped in and
/// the language set for it.
fn borrow(command: &ClangCommand, file: &Path) -> Option<Vec<String>> {
    let language = clang_language(file)?;
    let theirs = command.file.display().to_string();
    let mut args: Vec<String> = command.arguments.iter().map(|a| if *a == theirs { file.display().to_string() } else { a.clone() }).collect();
    match args.iter().position(|a| a == "-x") {
        Some(ix) if ix + 1 < args.len() => args[ix + 1] = language.to_string(),
        _ => args.splice(0..0, ["-x".to_string(), language.to_string()]).for_each(drop),
    }
    if !args.iter().any(|a| Path::new(a) == file) {
        args.push(file.display().to_string());
    }
    Some(args)
}

impl BuildGraph {
    pub fn build(inputs: &Inputs) -> Self {
        let store = &inputs.index_store;
        let root = real(&inputs.root);
        // The log's commands by target (or, from a database written before
        // targets were recorded, by Swift module).
        let mut logged: HashMap<String, Vec<&CompileCommand>> = HashMap::new();
        for command in &inputs.commands {
            let key = command.target.clone().or_else(|| flag_value(&command.arguments, "-module-name").map(str::to_string));
            if let Some(key) = key {
                logged.entry(key).or_default().push(command);
            }
        }
        let context = module_context(&inputs.commands);
        let mut targets = vec![];
        let mut used_keys = HashSet::new();
        for model in &inputs.model {
            let settings = inputs.build_settings.get(&model.name);
            let module = settings
                .and_then(|s| setting(s, "PRODUCT_MODULE_NAME").map(str::to_string))
                .unwrap_or_else(|| default_module_name(&model.name));
            let key = [model.name.clone(), module].into_iter().find(|key| logged.contains_key(key));
            let log: Vec<&CompileCommand> = key.as_ref().and_then(|k| logged.get(k)).cloned().unwrap_or_default();
            if let Some(key) = key {
                used_keys.insert(key);
            }
            let index = inputs.index_settings.get(&model.name);
            let mut target = Target {
                id: target_id(model.kind.unwrap_or(Kind::Library), Some(&model.project), &model.name),
                name: model.name.clone(),
                project: Some(model.project.clone()),
                kind: model.kind.unwrap_or(Kind::Library),
                directory: model.directory.clone(),
                files: model.files.clone(),
                generated: vec![],
                swift: None,
                swift_origin: None,
                clang: BTreeMap::new(),
                clang_template: None,
                clang_origin: None,
            };
            // Swift: Xcode's index arguments, the log's, or the settings'.
            let index_swift = index.and_then(|files| {
                let mut entries: Vec<(&PathBuf, &Vec<String>)> = files.iter().filter_map(|(f, a)| Some((f, a.swift.as_ref()?))).collect();
                entries.sort();
                entries.first().map(|(_, args)| (*args).clone())
            });
            let logged_swift = log.iter().find(|c| is_swift(Path::new(&c.file))).map(|c| c.arguments.clone());
            let (swift, origin) = match (index_swift, logged_swift) {
                // Xcode's point at the index build's products; modules a
                // build made elsewhere (⌘B, after packages were added) are
                // reached through the log's search paths too.
                (Some(args), _) => (Some(with_context(crate::build::compiledb::clean_arguments(&args), &context)), Some(Origin::IndexSettings)),
                (None, Some(args)) => (Some(args), Some(Origin::BuildLog)),
                (None, None) => match settings.and_then(|s| swift_from_settings(s, &context)) {
                    Some(mut args) => {
                        let at = args.iter().position(|a| a == "-import-objc-header").unwrap_or(args.len());
                        let dirs = model.header_dirs.iter().flat_map(|d| ["-Xcc".to_string(), format!("-I{}", d.display())]);
                        args.splice(at..at, dirs);
                        (Some(args), Some(Origin::BuildSettings))
                    }
                    None => (None, None),
                },
            };
            if let Some(args) = &swift {
                // Files the build generates, outside the project's folder.
                target.generated = swift_sources(args)
                    .iter()
                    .filter(|f| f.is_file())
                    .map(|f| real(f))
                    .filter(|f| !f.starts_with(&root) && !target.files.contains(f))
                    .collect();
                target.swift = Some(with_index_store(swift_template(args), store));
                target.swift_origin = origin;
            }
            // C family: each file's own, from Xcode's index arguments or the log.
            for (file, args) in index.into_iter().flatten() {
                if let Some(args) = &args.clang {
                    let command = ClangCommand { file: file.clone(), arguments: with_index_store(clang_arguments(args), store) };
                    target.clang.insert(real(file), command);
                    target.clang_origin = Some(Origin::IndexSettings);
                }
            }
            for command in log.iter().filter(|c| !is_swift(Path::new(&c.file))) {
                let file = real(Path::new(&command.file));
                if language_of(&file).is_some() && !target.clang.contains_key(&file) {
                    let arguments = with_index_store(clang_arguments(&command.arguments), store);
                    target.clang.insert(file.clone(), ClangCommand { file, arguments });
                    target.clang_origin.get_or_insert(Origin::BuildLog);
                }
            }
            target.clang_template = target.clang.values().next().cloned();
            if target.clang_template.is_none()
                && let Some(settings) = settings
                && let Some(file) = target.files.iter().find(|f| clang_language(f).is_some())
                && let Some(mut arguments) = clang_from_settings(settings, file)
            {
                let at = arguments.len().saturating_sub(2);
                arguments.splice(at..at, model.header_dirs.iter().map(|d| format!("-I{}", d.display())));
                target.clang_template = Some(ClangCommand { file: file.clone(), arguments: with_index_store(arguments, store) });
                target.clang_origin = Some(Origin::BuildSettings);
            }
            targets.push(target);
        }
        // Targets only the log knows: packages' and other projects'.
        let mut rest: Vec<(&String, &Vec<&CompileCommand>)> = logged.iter().filter(|(k, _)| !used_keys.contains(*k)).collect();
        rest.sort_by_key(|(k, _)| *k);
        for (name, log) in rest {
            let project = log.first().and_then(|c| c.project.clone());
            let mut files: Vec<PathBuf> = log.iter().map(|c| real(Path::new(&c.file))).collect();
            files.sort();
            files.dedup();
            let swift = log.iter().find(|c| is_swift(Path::new(&c.file))).map(|c| with_index_store(swift_template(&c.arguments), store));
            let clang: BTreeMap<PathBuf, ClangCommand> = log
                .iter()
                .filter(|c| language_of(Path::new(&c.file)).is_some_and(|l| l != "swift"))
                .map(|c| (real(Path::new(&c.file)), ClangCommand { file: c.file.clone().into(), arguments: with_index_store(clang_arguments(&c.arguments), store) }))
                .collect();
            let directory = log.first().map(|c| PathBuf::from(&c.directory)).unwrap_or_default();
            targets.push(Target {
                id: target_id(Kind::Dependency, project.as_deref(), name),
                name: name.clone(),
                project,
                kind: Kind::Dependency,
                directory,
                files,
                generated: vec![],
                swift_origin: swift.as_ref().map(|_| Origin::BuildLog),
                swift,
                clang_template: clang.values().next().cloned(),
                clang_origin: (!clang.is_empty()).then_some(Origin::BuildLog),
                clang,
            });
        }
        targets.sort_by(|a, b| a.id.cmp(&b.id));
        Self {
            targets,
            index_store: inputs.index_store.clone(),
            index_database: inputs.index_database.clone(),
            default_sdk: inputs.default_sdk.clone(),
        }
    }

    pub fn target(&self, id: &str) -> Option<&Target> {
        self.targets.iter().find(|t| t.id == id)
    }

    /// The targets `file` is in.
    pub fn targets_of(&self, file: &Path) -> Vec<&Target> {
        self.targets.iter().filter(|t| t.files.iter().any(|f| f == file) || t.generated.iter().any(|f| f == file)).collect()
    }

    /// The arguments for `file` in target `id`; `language` is what it was
    /// opened as. `None` for a header (sourcekit-lsp borrows a file's that
    /// includes it).
    pub fn options(&self, id: &str, file: &Path, language: Option<&str>) -> Option<Options> {
        let target = self.target(id)?;
        let swift = match language {
            Some(language) => language == "swift",
            None => is_swift(file),
        };
        let alone = |flag: &str| -> Option<Options> {
            if is_header(file) || (!swift && clang_language(file).is_none()) {
                return None;
            }
            let mut arguments: Vec<String> = match &self.default_sdk {
                Some(sdk) => vec![flag.to_string(), sdk.display().to_string()],
                None => vec![],
            };
            if !swift {
                arguments.splice(0..0, ["-x".to_string(), clang_language(file)?.to_string()]);
            }
            arguments.push(file.display().to_string());
            Some(Options { arguments, working_directory: target.directory.clone(), origin: Origin::FileAlone })
        };
        let (arguments, origin) = if swift {
            let Some(template) = target.swift.clone() else { return alone("-sdk") };
            // The target's files, the asked-for one as sourcekit-lsp names it
            // (swiftc refuses two paths to one file). Not the ones missing
            // from disk (a build would generate them): one input the
            // compiler can't open and it gives up on every file.
            let mut arguments = template;
            let asked = real(file);
            let mut listed = false;
            for f in target.files.iter().chain(&target.generated).filter(|f| is_swift(f) && (*f == &asked || f.exists())) {
                let this = *f == asked || f == file;
                listed |= this;
                arguments.push(if this { file } else { f }.display().to_string());
            }
            if !listed {
                arguments.push(file.display().to_string());
            }
            (arguments, target.swift_origin?)
        } else if let Some(command) = target.clang.get(file) {
            (command.arguments.clone(), target.clang_origin?)
        } else {
            let Some(template) = target.clang_template.as_ref().filter(|_| !is_header(file)) else { return alone("-isysroot") };
            (borrow(template, file)?, target.clang_origin?)
        };
        Some(Options { arguments, working_directory: target.directory.clone(), origin })
    }

    /// The targets whose files or arguments differ from `old`'s; `None`
    /// when targets came or went (everything changed).
    pub fn changed_since(&self, old: &BuildGraph) -> Option<Vec<String>> {
        if self.targets.len() != old.targets.len() || self.targets.iter().zip(&old.targets).any(|(a, b)| a.id != b.id) {
            return None;
        }
        if self.index_store != old.index_store {
            return None;
        }
        Some(self.targets.iter().zip(&old.targets).filter(|(a, b)| a != b).map(|(a, _)| a.id.clone()).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    /// A project at `/p` with an app (A.swift, B.swift, c.m, d.h) and its
    /// tests, and a scratch folder for files that must exist.
    fn inputs(scratch: &Path) -> Inputs {
        Inputs {
            root: PathBuf::from("/p"),
            model: vec![
                ModelTarget {
                    name: "App".into(),
                    project: "App".into(),
                    kind: Some(Kind::Application),
                    files: ["/p/App/A.swift", "/p/App/B.swift", "/p/App/c.m", "/p/App/d.h"].map(PathBuf::from).to_vec(),
                    directory: "/p".into(),
                    header_dirs: vec![],
                },
                ModelTarget {
                    name: "AppTests".into(),
                    project: "App".into(),
                    kind: Some(Kind::Test),
                    files: vec!["/p/AppTests/T.swift".into()],
                    directory: "/p".into(),
                    header_dirs: vec![],
                },
            ],
            commands: vec![
                CompileCommand {
                    directory: "/p".into(),
                    file: "/p/App/A.swift".into(),
                    arguments: vec![
                        "/usr/bin/swiftc".into(),
                        "-module-name".into(),
                        "App".into(),
                        "/p/App/A.swift".into(),
                        "/p/App/Gone.swift".into(),
                        scratch.join("GeneratedAssetSymbols.swift").display().to_string(),
                        "-sdk".into(),
                        "/SDK".into(),
                        "-I".into(),
                        "/dd/Build/Products/Debug-iphonesimulator".into(),
                        "-Xcc".into(),
                        "-fmodule-map-file=/dd/GeneratedModuleMaps/Kingfisher.modulemap".into(),
                        "-index-store-path".into(),
                        "/elsewhere".into(),
                    ],
                    target: Some("App".into()),
                    project: Some("App".into()),
                },
                CompileCommand {
                    directory: "/p".into(),
                    file: "/p/App/c.m".into(),
                    arguments: strings(&["/usr/bin/clang", "-x", "objective-c", "-fobjc-arc", "-c", "/p/App/c.m", "-o", "/dd/c.o"]),
                    target: Some("App".into()),
                    project: Some("App".into()),
                },
                CompileCommand {
                    directory: "/dd/SourcePackages/checkouts/Kingfisher".into(),
                    file: "/dd/SourcePackages/checkouts/Kingfisher/Sources/K.swift".into(),
                    arguments: strings(&["/usr/bin/swiftc", "-module-name", "Kingfisher", "/dd/SourcePackages/checkouts/Kingfisher/Sources/K.swift"]),
                    target: Some("Kingfisher".into()),
                    project: Some("Kingfisher".into()),
                },
            ],
            index_store: "/dd/Index.noindex/DataStore".into(),
            index_database: "/dd/Index.noindex/IndexDatabase".into(),
            ..Default::default()
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zcode-bsp-graph-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("GeneratedAssetSymbols.swift"), "").unwrap();
        dir
    }

    fn app(graph: &BuildGraph) -> &Target {
        graph.targets.iter().find(|t| t.name == "App").unwrap()
    }

    #[test]
    fn swift_files_get_their_targets_arguments_and_current_files() {
        let dir = scratch("swift");
        // A project on disk: A and B exist, Missing (a file a build
        // generates) doesn't yet.
        let project = real(&dir).join("proj");
        std::fs::create_dir_all(project.join("App")).unwrap();
        for file in ["A.swift", "B.swift"] {
            std::fs::write(project.join("App").join(file), "").unwrap();
        }
        let at = |relative: &str| project.join(relative).display().to_string();
        let mut inputs = inputs(&dir);
        inputs.root = project.clone();
        inputs.model[0].directory = project.clone();
        inputs.model[0].files = ["App/A.swift", "App/B.swift", "App/Missing.swift", "App/c.m"].map(|f| project.join(f)).to_vec();
        for arg in &mut inputs.commands[0].arguments {
            if let Some(name) = arg.strip_prefix("/p/") {
                *arg = at(name);
            }
        }
        let graph = BuildGraph::build(&inputs);
        let app = app(&graph);
        assert_eq!(app.swift_origin, Some(Origin::BuildLog));
        let generated = real(&dir.join("GeneratedAssetSymbols.swift"));
        assert_eq!(app.generated, [generated.clone()]);
        let options = graph.options(&app.id, Path::new(&at("App/B.swift")), Some("swift")).unwrap();
        let args = &options.arguments;
        // No compiler, the index build's store, the target's files now (B is
        // new since the build; Gone was removed; Missing isn't there) and
        // the generated one.
        assert_eq!(args[..3], strings(&["-module-name", "App", "-sdk"]));
        assert_eq!(flag_value(args, "-index-store-path"), Some("/dd/Index.noindex/DataStore"));
        for file in [at("App/A.swift"), at("App/B.swift")] {
            assert_eq!(args.iter().filter(|a| **a == file).count(), 1, "{file} in {args:?}");
        }
        assert!(!args.contains(&at("App/Gone.swift")) && !args.contains(&at("App/Missing.swift")), "{args:?}");
        assert!(args.contains(&generated.display().to_string()));
        assert_eq!(options.working_directory, project);
        // A file the project doesn't list yet is compiled with the rest.
        let loose = graph.options(&app.id, Path::new(&at("App/New.swift")), None).unwrap();
        assert!(loose.arguments.contains(&at("App/New.swift")) && loose.arguments.contains(&at("App/A.swift")));
        // A file asked for through a symlink is listed once, as asked.
        let through_link = Path::new("/").join(generated.strip_prefix("/private").unwrap_or(&generated));
        if through_link != generated {
            let args = graph.options(&app.id, &through_link, Some("swift")).unwrap().arguments;
            assert_eq!(args.iter().filter(|a| a.ends_with("GeneratedAssetSymbols.swift")).count(), 1, "{args:?}");
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn xcodes_index_arguments_come_first() {
        let dir = scratch("index");
        let mut inputs = inputs(&dir);
        let index = HashMap::from([(
            "App".to_string(),
            HashMap::from([(
                PathBuf::from("/p/App/A.swift"),
                IndexArguments {
                    swift: Some(strings(&["-module-name", "App", "/p/App/A.swift", "/p/App/B.swift", "-DINDEX", "-c", "-j16", "-sdk", "/SDK"])),
                    clang: None,
                },
            )]),
        )]);
        inputs.index_settings = Arc::new(index);
        let graph = BuildGraph::build(&inputs);
        let app = app(&graph);
        assert_eq!(app.swift_origin, Some(Origin::IndexSettings));
        let args = graph.options(&app.id, Path::new("/p/App/A.swift"), Some("swift")).unwrap().arguments;
        assert!(args.contains(&"-DINDEX".to_string()) && !args.contains(&"-c".to_string()) && !args.contains(&"-j16".to_string()), "{args:?}");
        // With the modules the logged builds made, once.
        assert_eq!(args.windows(2).filter(|w| *w == ["-I", "/dd/Build/Products/Debug-iphonesimulator"]).count(), 1, "{args:?}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_target_nothing_compiled_gets_arguments_from_its_settings() {
        let dir = scratch("settings");
        let mut inputs = inputs(&dir);
        let settings: HashMap<String, String> = [
            ("PRODUCT_MODULE_NAME", "AppTests"),
            ("SDKROOT", "/SDK"),
            ("CURRENT_ARCH", "undefined_arch"),
            ("NATIVE_ARCH", "arm64"),
            ("LLVM_TARGET_TRIPLE_OS_VERSION", "ios15.0"),
            ("LLVM_TARGET_TRIPLE_SUFFIX", "-simulator"),
            ("SWIFT_VERSION", "5.0"),
            ("SWIFT_ACTIVE_COMPILATION_CONDITIONS", "DEBUG MOCKABLE"),
            ("OTHER_SWIFT_FLAGS", "-DMOZ_TARGET_CLIENT"),
            ("SWIFT_OBJC_BRIDGING_HEADER", "App/Bridging.h"),
            ("PROJECT_DIR", "/p"),
            ("BUILT_PRODUCTS_DIR", "/dd/Build/Products/Debug-iphonesimulator"),
            ("HEADER_SEARCH_PATHS", "\"/p/with space\" $(inherited)"),
            ("SWIFT_UPCOMING_FEATURE_EXISTENTIAL_ANY", "YES"),
            ("SWIFT_UPCOMING_FEATURE_DISABLE_OUTWARD_ACTOR_ISOLATION", "YES"),
            ("SWIFT_UPCOMING_FEATURE_CONCISE_MAGIC_FILE", "NO"),
            ("SWIFT_STRICT_CONCURRENCY", "complete"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        inputs.build_settings = Arc::new(HashMap::from([("AppTests".to_string(), settings)]));
        inputs.model[1].header_dirs = vec!["/p/App/Headers".into()];
        let graph = BuildGraph::build(&inputs);
        let tests = graph.targets.iter().find(|t| t.name == "AppTests").unwrap();
        assert_eq!(tests.swift_origin, Some(Origin::BuildSettings));
        let args = graph.options(&tests.id, Path::new("/p/AppTests/T.swift"), Some("swift")).unwrap().arguments;
        assert_eq!(flag_value(&args, "-module-name"), Some("AppTests"));
        assert_eq!(flag_value(&args, "-target"), Some("arm64-apple-ios15.0-simulator"));
        assert_eq!(flag_value(&args, "-swift-version"), Some("5"));
        assert_eq!(flag_value(&args, "-import-objc-header"), Some("/p/App/Bridging.h"));
        for expected in ["-DDEBUG", "-DMOCKABLE", "-DMOZ_TARGET_CLIENT", "-strict-concurrency=complete", "-I/p/with space", "/p/AppTests/T.swift"] {
            assert!(args.contains(&expected.to_string()), "{expected} in {args:?}");
        }
        let features: Vec<&str> = args.iter().enumerate().filter(|(_, a)| *a == "-enable-upcoming-feature").map(|(i, _)| args[i + 1].as_str()).collect();
        assert_eq!(features, ["DisableOutwardActorInference", "ExistentialAny"]);
        // The project's headers, as Xcode's header maps reach them, before the bridging header.
        let headers = args.iter().position(|a| a == "-I/p/App/Headers").unwrap();
        assert!(args[headers - 1] == "-Xcc" && headers < args.iter().position(|a| a == "-import-objc-header").unwrap(), "{args:?}");
        // The modules the index build made: products and packages' module maps.
        assert!(args.windows(2).any(|w| w == ["-I", "/dd/Build/Products/Debug-iphonesimulator"]), "{args:?}");
        assert!(args.contains(&"-fmodule-map-file=/dd/GeneratedModuleMaps/Kingfisher.modulemap".to_string()), "{args:?}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn c_family_files_have_their_own_or_borrow_a_siblings() {
        let dir = scratch("clang");
        let graph = BuildGraph::build(&inputs(&dir));
        let app = app(&graph);
        let own = graph.options(&app.id, Path::new("/p/App/c.m"), Some("objective-c")).unwrap().arguments;
        assert_eq!(own[..3], strings(&["-x", "objective-c", "-fobjc-arc"]));
        assert!(!own.contains(&"-o".to_string()) && !own.contains(&"-c".to_string()), "{own:?}");
        let borrowed = graph.options(&app.id, Path::new("/p/App/e.c"), Some("c")).unwrap().arguments;
        assert_eq!(borrowed[..2], strings(&["-x", "c"]));
        assert!(borrowed.contains(&"/p/App/e.c".to_string()) && !borrowed.contains(&"/p/App/c.m".to_string()), "{borrowed:?}");
        // Headers borrow from a file that includes them, in sourcekit-lsp.
        assert_eq!(graph.options(&app.id, Path::new("/p/App/d.h"), None), None);
        // A target nothing compiled has its files alone, with the default SDK.
        let mut inputs = inputs(&dir);
        inputs.default_sdk = Some("/MacOSX.sdk".into());
        let graph = BuildGraph::build(&inputs);
        let tests = graph.targets.iter().find(|t| t.name == "AppTests").unwrap();
        let alone = graph.options(&tests.id, Path::new("/p/AppTests/T.swift"), Some("swift")).unwrap();
        assert_eq!((alone.arguments, alone.origin), (strings(&["-sdk", "/MacOSX.sdk", "/p/AppTests/T.swift"]), Origin::FileAlone));
        let alone = graph.options(&tests.id, Path::new("/p/AppTests/t.c"), Some("c")).unwrap();
        assert_eq!(alone.arguments, strings(&["-x", "c", "-isysroot", "/MacOSX.sdk", "/p/AppTests/t.c"]));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn targets_sort_apps_first_and_packages_last() {
        let dir = scratch("order");
        let graph = BuildGraph::build(&inputs(&dir));
        let names: Vec<(&str, Kind)> = graph.targets.iter().map(|t| (t.name.as_str(), t.kind)).collect();
        assert_eq!(names, [("App", Kind::Application), ("AppTests", Kind::Test), ("Kingfisher", Kind::Dependency)]);
        let kingfisher = &graph.targets[2];
        assert_eq!(kingfisher.files, [PathBuf::from("/dd/SourcePackages/checkouts/Kingfisher/Sources/K.swift")]);
        assert!(graph.options(&kingfisher.id, &kingfisher.files[0], None).is_some());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_database_without_targets_is_grouped_by_module() {
        let dir = scratch("modules");
        let mut inputs = inputs(&dir);
        for command in &mut inputs.commands {
            command.target = None;
        }
        let graph = BuildGraph::build(&inputs);
        assert_eq!(app(&graph).swift_origin, Some(Origin::BuildLog));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn says_which_targets_changed() {
        let dir = scratch("changes");
        let mut inputs = inputs(&dir);
        let before = BuildGraph::build(&inputs);
        assert_eq!(BuildGraph::build(&inputs).changed_since(&before), Some(vec![]));
        // A file joins the app.
        inputs.model[0].files.push("/p/App/New.swift".into());
        let after = BuildGraph::build(&inputs);
        assert_eq!(after.changed_since(&before), Some(vec![app(&after).id.clone()]));
        // A target is added: everything.
        inputs.model.push(ModelTarget { name: "Widget".into(), project: "App".into(), ..Default::default() });
        assert_eq!(BuildGraph::build(&inputs).changed_since(&after), None);
        std::fs::remove_dir_all(dir).ok();
    }
}

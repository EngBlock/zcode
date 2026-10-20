//! A web app's `package.json` scripts as run targets beside the project's
//! schemes: finding the web apps and their scripts, the package manager
//! their lockfile implies, the command a script runs as, and what its
//! output says (the dev server's URL, errors as issues). The runs
//! themselves are terminals in the window's debug area
//! (`workspace/web_runs.rs`).

use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::log::{Issue, IssueSource, Severity};

/// Issues kept per run; a broken build can print thousands.
const MAX_ISSUES: usize = 500;
/// How many lines after an error its location may come (Vite prints the
/// code frame first).
const LOCATION_LINES: u8 = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageManager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl PackageManager {
    pub fn name(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        [Self::Npm, Self::Pnpm, Self::Yarn, Self::Bun].into_iter().find(|m| m.name() == name)
    }

    /// The manager a web app uses: `package.json`'s `packageManager`
    /// (Corepack's `pnpm@9.1.0`), else the lockfile in its folder or the
    /// nearest folder above it in the project (a monorepo keeps one at its
    /// root), else npm.
    pub fn detect(dir: &Path, project_root: &Path, declared: Option<&str>) -> Self {
        if let Some(manager) = declared.and_then(|d| Self::from_name(d.split('@').next().unwrap_or(d).trim())) {
            return manager;
        }
        const LOCKFILES: &[(&str, PackageManager)] = &[
            ("bun.lock", PackageManager::Bun),
            ("bun.lockb", PackageManager::Bun),
            ("pnpm-lock.yaml", PackageManager::Pnpm),
            ("yarn.lock", PackageManager::Yarn),
            ("package-lock.json", PackageManager::Npm),
            ("npm-shrinkwrap.json", PackageManager::Npm),
        ];
        for folder in dir.ancestors() {
            if let Some((_, manager)) = LOCKFILES.iter().find(|(file, _)| folder.join(file).is_file()) {
                return *manager;
            }
            if folder == project_root || !folder.starts_with(project_root) {
                break;
            }
        }
        Self::Npm
    }

    /// The shell command that runs `script` (`exec`, so the package
    /// manager leads the run's processes).
    pub fn run_command(self, script: &str) -> String {
        let quoted = shlex::try_quote(script).map(|q| q.into_owned()).unwrap_or_else(|_| script.to_string());
        format!("exec {} run {quoted}", self.name())
    }
}

impl fmt::Display for PackageManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Script {
    pub name: String,
    /// What `package.json` says it runs (`vite`).
    pub command: String,
}

/// A folder with a `package.json` that has scripts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebApp {
    /// Relative to the project's folder (empty: the project's own).
    pub folder: PathBuf,
    pub dir: PathBuf,
    /// What menus call it: its folder (`web`), or the project's name.
    pub label: String,
    pub manager: PackageManager,
    /// In `package.json`'s order.
    pub scripts: Vec<Script>,
}

impl WebApp {
    pub fn id(&self, script: &str) -> ScriptId {
        ScriptId { folder: self.folder.clone(), script: script.to_string() }
    }

    pub fn script(&self, name: &str) -> Option<&Script> {
        self.scripts.iter().find(|s| s.name == name)
    }

    /// `web: dev`: how a script is named where only a name fits (MCP, the
    /// debug area's terminal).
    pub fn script_name(&self, script: &str) -> String {
        format!("{}: {script}", self.label)
    }

    /// The `PATH` folders a script finds its packages' programs in: its
    /// own `node_modules/.bin`, then those of the folders above it in the
    /// project (a monorepo's hoisted ones).
    pub fn bin_dirs(&self, project_root: &Path) -> Vec<PathBuf> {
        let mut dirs = vec![];
        for folder in self.dir.ancestors() {
            let bin = folder.join("node_modules").join(".bin");
            if bin.is_dir() || folder == self.dir {
                dirs.push(bin);
            }
            if folder == project_root || !folder.starts_with(project_root) {
                break;
            }
        }
        dirs
    }
}

/// A script of a web app, as the selection and the session keep it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScriptId {
    /// The web app's folder, relative to the project's.
    pub folder: PathBuf,
    pub script: String,
}

/// The web apps among the project's files (`files` relative to `root`, as
/// the project's scan lists them): every `package.json` outside
/// `node_modules` with scripts, the project's own first, then by folder.
/// Reads the files: off the main thread.
pub fn discover(root: &Path, files: &[PathBuf]) -> Vec<WebApp> {
    let mut apps: Vec<WebApp> = files
        .iter()
        .filter(|f| f.file_name().is_some_and(|n| n == "package.json"))
        .filter(|f| !f.components().any(|c| c == Component::Normal("node_modules".as_ref())))
        .filter_map(|f| read_app(root, f.parent().unwrap_or(Path::new(""))))
        .collect();
    apps.sort_by(|a, b| a.folder.components().count().min(1).cmp(&b.folder.components().count().min(1)).then(a.folder.cmp(&b.folder)));
    apps
}

/// The web app in `folder` (relative to `root`), when its `package.json`
/// has scripts.
pub fn read_app(root: &Path, folder: &Path) -> Option<WebApp> {
    let dir = root.join(folder);
    let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let manifest: Manifest = serde_json::from_str(&text).ok()?;
    let scripts = manifest.scripts.0;
    if scripts.is_empty() {
        return None;
    }
    let label = if folder.as_os_str().is_empty() {
        root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "package.json".into())
    } else {
        folder.to_string_lossy().into_owned()
    };
    let manager = PackageManager::detect(&dir, root, manifest.package_manager.as_deref());
    Some(WebApp { folder: folder.to_path_buf(), dir, label, manager, scripts })
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Manifest {
    scripts: OrderedScripts,
    #[serde(rename = "packageManager")]
    package_manager: Option<String>,
}

/// `scripts` in the file's order (serde_json's maps sort their keys).
#[derive(Default)]
struct OrderedScripts(Vec<Script>);

impl<'de> Deserialize<'de> for OrderedScripts {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = OrderedScripts;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an object of scripts")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<OrderedScripts, A::Error> {
                let mut scripts = vec![];
                while let Some((name, value)) = map.next_entry::<String, serde_json::Value>()? {
                    // Only strings are scripts; npm ignores the rest.
                    if let serde_json::Value::String(command) = value {
                        scripts.push(Script { name, command });
                    }
                }
                Ok(OrderedScripts(scripts))
            }
            fn visit_unit<E>(self) -> Result<OrderedScripts, E> {
                Ok(OrderedScripts::default())
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

// ---------------------------------------------------------------------------
// What a script prints

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid regex"))
}

/// The first URL on this Mac in `line` (ANSI already stripped): Vite's
/// `Local:   http://localhost:5173/`, Next's, webpack-dev-server's
/// `Loopback:`, `astro dev`'s, any `http://localhost:NNNN` or
/// `127.0.0.1`. `0.0.0.0` and `[::]` (every interface) become
/// `localhost`.
pub fn local_url(line: &str) -> Option<String> {
    static URL: OnceLock<Regex> = OnceLock::new();
    let found = re(&URL, r"(?i)\bhttps?://(localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1?\]|[a-z0-9-]+\.localhost)(:\d{2,5})?(/[^\s'\x22<>)\]]*)?").captures(line)?;
    let whole = found.get(0)?.as_str().trim_end_matches(['.', ',', ';', ':']);
    let host = &found[1];
    Some(match host {
        "0.0.0.0" | "[::]" => whole.replacen(host, "localhost", 1),
        _ => whole.to_string(),
    })
}

/// A script's output read as it comes: the dev server's URL and the
/// errors and warnings it printed, with paths resolved against the web
/// app's folder.
#[derive(Clone, Debug, Default)]
pub struct OutputParser {
    dir: PathBuf,
    pub url: Option<String>,
    pub issues: Vec<Issue>,
    /// An issue whose location comes on a later line (esbuild, Vite), and
    /// how many lines may still come before it.
    awaiting_location: Option<(usize, u8)>,
    /// ESLint's stylish format: the file the indented lines are in.
    eslint_file: Option<PathBuf>,
}

/// What a line changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fed {
    pub url: bool,
    pub issues: bool,
}

impl OutputParser {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, ..Default::default() }
    }

    pub fn feed(&mut self, line: &str) -> Fed {
        let mut fed = Fed::default();
        let line = line.trim_end();
        if self.url.is_none()
            && let Some(url) = local_url(line)
        {
            self.url = Some(url);
            fed.url = true;
        }
        if restarts(line) {
            fed.issues = !self.issues.is_empty();
            self.issues.clear();
            self.awaiting_location = None;
            return fed;
        }
        if let Some((ix, left)) = self.awaiting_location.take() {
            if let Some((path, line_number, column)) = self.location(line) {
                if let Some(issue) = self.issues.get_mut(ix) {
                    issue.path = Some(path);
                    issue.line = Some(line_number);
                    issue.column = Some(column);
                    fed.issues = true;
                }
                return fed;
            }
            if left > 1 {
                self.awaiting_location = Some((ix, left - 1));
            }
        }
        if let Some((issue, needs_location)) = self.issue(line)
            && !self.issues.contains(&issue)
            && self.issues.len() < MAX_ISSUES
        {
            self.issues.push(issue);
            fed.issues = true;
            if needs_location {
                self.awaiting_location = Some((self.issues.len() - 1, LOCATION_LINES));
            }
        }
        fed
    }

    fn path(&self, path: &str) -> PathBuf {
        let path = Path::new(path.trim());
        if path.is_absolute() { path.to_path_buf() } else { self.dir.join(path) }
    }

    fn issue_at(&self, severity: &str, message: &str, path: &str, line: &str, column: Option<&str>) -> Issue {
        Issue {
            severity: severity_of(severity),
            message: message.trim().to_string(),
            path: Some(self.path(path)),
            line: line.parse().ok(),
            column: column.and_then(|c| c.parse().ok()),
            source: IssueSource::Build,
        }
    }

    /// A location on a line of its own, for the issue before it: esbuild's
    /// indented `src/main.ts:3:7:`, Vite's `File: /…/App.vue:3:7`.
    fn location(&self, line: &str) -> Option<(PathBuf, u32, u32)> {
        static ESBUILD: OnceLock<Regex> = OnceLock::new();
        static VITE: OnceLock<Regex> = OnceLock::new();
        let found = re(&ESBUILD, r"^\s+([^\s:][^:]*):(\d+):(\d+):$")
            .captures(line)
            .or_else(|| re(&VITE, r"^\s*File: (.+?):(\d+):(\d+)\s*$").captures(line))?;
        Some((self.path(&found[1]), found[2].parse().ok()?, found[3].parse().ok()?))
    }

    /// The issue a line reports, and whether its location comes later.
    fn issue(&mut self, line: &str) -> Option<(Issue, bool)> {
        static TSC: OnceLock<Regex> = OnceLock::new();
        static TSC_PRETTY: OnceLock<Regex> = OnceLock::new();
        static COMPILER: OnceLock<Regex> = OnceLock::new();
        static ESLINT_UNIX: OnceLock<Regex> = OnceLock::new();
        static ESLINT_FILE: OnceLock<Regex> = OnceLock::new();
        static ESLINT_ENTRY: OnceLock<Regex> = OnceLock::new();
        static ESBUILD: OnceLock<Regex> = OnceLock::new();
        static VITE: OnceLock<Regex> = OnceLock::new();
        static VITE_LOCATED: OnceLock<Regex> = OnceLock::new();
        static VITE_IMPORT: OnceLock<Regex> = OnceLock::new();

        // tsc (and vue-tsc): `src/main.ts(12,5): error TS2322: …`.
        if let Some(c) = re(&TSC, r"^([^\s(][^(]*)\((\d+),(\d+)\): (error|warning) TS\d+: (.+)$").captures(line) {
            return Some((self.issue_at(&c[4], &c[5], &c[1], &c[2], Some(&c[3])), false));
        }
        // tsc --pretty: `src/main.ts:12:5 - error TS2322: …`.
        if let Some(c) = re(&TSC_PRETTY, r"^(\S[^:]*):(\d+):(\d+) - (error|warning) TS\d+: (.+)$").captures(line) {
            return Some((self.issue_at(&c[4], &c[5], &c[1], &c[2], Some(&c[3])), false));
        }
        // ESLint's unix format: `src/a.js:3:7: 'x' is unused [Error/no-unused-vars]`.
        if let Some(c) = re(&ESLINT_UNIX, r"^([^\s:][^:]*):(\d+):(\d+): (.+) \[(Error|Warning)/[^\]]+\]$").captures(line) {
            return Some((self.issue_at(&c[5], &c[4], &c[1], &c[2], Some(&c[3])), false));
        }
        // Compilers' `path:line:col: error: …` (esbuild's transform errors
        // say `ERROR:`).
        if let Some(c) = re(&COMPILER, r"(?i)^([^\s:][^:]*):(\d+):(\d+): (error|warning|fatal error): (.+)$").captures(line) {
            return Some((self.issue_at(&c[4], &c[5], &c[1], &c[2], Some(&c[3])), false));
        }
        // esbuild: `✘ [ERROR] Expected ";" but found "b"`, located below.
        if let Some(c) = re(&ESBUILD, r"^\s*(?:✘|X|▲) \[(ERROR|WARNING)\] (.+)$").captures(line) {
            let issue = Issue { severity: severity_of(&c[1]), message: c[2].trim().to_string(), path: None, line: None, column: None, source: IssueSource::Build };
            return Some((issue, true));
        }
        // Vite: `[vite] Internal server error: /…/App.vue:3:7: …`, or the
        // message with `File: …` below.
        if let Some(c) = re(&VITE, r"\[vite\] (?:\(\w+\) )?Internal server error: (.+)$").captures(line) {
            let rest = c[1].trim();
            if let Some(l) = re(&VITE_LOCATED, r"^(/[^:]+):(\d+):(\d+):?\s*(.*)$").captures(rest) {
                let message = if l[4].is_empty() { rest } else { &l[4] };
                return Some((self.issue_at("error", message, &l[1], &l[2], Some(&l[3])), false));
            }
            let mut issue = Issue { severity: Severity::Error, message: rest.to_string(), path: None, line: None, column: None, source: IssueSource::Build };
            // `Failed to resolve import "./x" from "src/main.ts"`.
            if let Some(i) = re(&VITE_IMPORT, r#" from "([^"]+)""#).captures(rest) {
                issue.path = Some(self.path(&i[1]));
            }
            return Some((issue, true));
        }
        // ESLint's stylish format: the file's path, then indented
        // `3:7  error  'x' is unused  no-unused-vars` lines.
        if line.is_empty() {
            self.eslint_file = None;
            return None;
        }
        if re(&ESLINT_FILE, r"^/\S.*\.\w+$").is_match(line) {
            self.eslint_file = Some(PathBuf::from(line));
            return None;
        }
        if let Some(file) = &self.eslint_file
            && let Some(c) = re(&ESLINT_ENTRY, r"^\s+(\d+):(\d+)\s+(error|warning)\s+(.+?)(?:\s{2,}[\w@/.-]+)?$").captures(line)
        {
            let file = file.to_string_lossy().into_owned();
            return Some((self.issue_at(&c[3], &c[4], &file, &c[1], Some(&c[2])), false));
        }
        None
    }
}

fn severity_of(word: &str) -> Severity {
    match word.to_ascii_lowercase().as_str() {
        "warning" => Severity::Warning,
        _ => Severity::Error,
    }
}

/// A watcher starting over: what it reported before is out of date (tsc
/// `--watch`, Vite's hot updates).
fn restarts(line: &str) -> bool {
    line.contains("File change detected. Starting incremental compilation")
        || line.contains("Starting compilation in watch mode")
        || line.contains("hmr update ")
        || line.contains("[vite] page reload")
        || line.contains("[vite] (client) page reload")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zcode-web-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        for (file, text) in files {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
        }
        dir
    }

    #[test]
    fn web_apps_are_the_package_json_files_with_scripts_outside_node_modules() {
        let root = tree(
            "discover",
            &[
                ("package.json", r#"{"name":"root","workspaces":["web"],"scripts":{"lint":"eslint ."}}"#),
                ("web/package.json", r#"{"scripts":{"dev":"vite","build":"vue-tsc -b && vite build","preview":"vite preview","n":1}}"#),
                ("web/node_modules/vite/package.json", r#"{"scripts":{"postinstall":"x"}}"#),
                ("docs/package.json", r#"{"name":"docs"}"#),
                ("site/package.json", "not json"),
            ],
        );
        let files: Vec<PathBuf> = ["package.json", "web/package.json", "web/node_modules/vite/package.json", "docs/package.json", "site/package.json", "web/src/main.ts"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let apps = discover(&root, &files);
        let labels: Vec<&str> = apps.iter().map(|a| a.label.as_str()).collect();
        let root_name = root.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(labels, [root_name.as_str(), "web"]);
        let web = &apps[1];
        assert_eq!(web.dir, root.join("web"));
        // In the file's order, strings only.
        let names: Vec<&str> = web.scripts.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["dev", "build", "preview"]);
        assert_eq!(web.script("dev").unwrap().command, "vite");
        assert_eq!(web.script_name("dev"), "web: dev");
        assert_eq!(web.id("dev"), ScriptId { folder: "web".into(), script: "dev".into() });
        assert_eq!(read_app(&root, Path::new("docs")), None, "no scripts");
    }

    #[test]
    fn the_package_manager_is_the_lockfile_s_or_the_declared_one_else_npm() {
        let root = tree(
            "managers",
            &[
                ("a/package.json", "{}"),
                ("b/package.json", "{}"),
                ("b/pnpm-lock.yaml", ""),
                ("c/package.json", "{}"),
                ("c/yarn.lock", ""),
                ("d/bun.lockb", ""),
                ("e/package-lock.json", ""),
                ("mono/bun.lock", ""),
                ("mono/apps/site/package.json", "{}"),
            ],
        );
        let detect = |dir: &str| PackageManager::detect(&root.join(dir), &root, None);
        assert_eq!(detect("a"), PackageManager::Npm, "nothing says otherwise");
        assert_eq!(detect("b"), PackageManager::Pnpm);
        assert_eq!(detect("c"), PackageManager::Yarn);
        assert_eq!(detect("d"), PackageManager::Bun);
        assert_eq!(detect("e"), PackageManager::Npm);
        assert_eq!(PackageManager::detect(&root.join("mono/apps/site"), &root.join("mono"), None), PackageManager::Bun, "the monorepo's root lockfile");
        assert_eq!(PackageManager::detect(&root.join("b"), &root, Some("yarn@4.1.0")), PackageManager::Yarn, "packageManager wins");
        assert_eq!(PackageManager::detect(&root.join("a"), &root, Some("deno@2")), PackageManager::Npm);
        // Lockfiles outside the project don't count.
        assert_eq!(PackageManager::detect(&root.join("mono/apps/site"), &root.join("mono/apps/site"), None), PackageManager::Npm);
        assert_eq!(PackageManager::Pnpm.run_command("dev"), "exec pnpm run dev");
        assert_eq!(PackageManager::Npm.run_command("dev:ios"), "exec npm run dev:ios");
        assert_eq!(PackageManager::Yarn.run_command("a b"), "exec yarn run 'a b'");
    }

    #[test]
    fn a_script_finds_its_own_and_its_monorepo_s_programs() {
        let root = tree("bins", &[("node_modules/.bin/tsc", ""), ("apps/site/package.json", "{}")]);
        let app = WebApp { folder: "apps/site".into(), dir: root.join("apps/site"), label: "apps/site".into(), manager: PackageManager::Npm, scripts: vec![] };
        assert_eq!(app.bin_dirs(&root), [root.join("apps/site/node_modules/.bin"), root.join("node_modules/.bin")]);
    }

    #[test]
    fn dev_servers_urls() {
        let cases = [
            ("  ➜  Local:   http://localhost:5173/", Some("http://localhost:5173/")),
            ("  ➜  Network: use --host to expose", None),
            ("   - Local:        http://localhost:3000", Some("http://localhost:3000")),
            ("<i> [webpack-dev-server] Loopback: http://localhost:8080/, http://[::1]:8080/", Some("http://localhost:8080/")),
            ("┃ Local    http://localhost:4321/", Some("http://localhost:4321/")),
            ("Serving HTTP on 0.0.0.0 port 8000 (http://0.0.0.0:8000/) ...", Some("http://localhost:8000/")),
            ("listening at http://127.0.0.1:4000.", Some("http://127.0.0.1:4000")),
            ("see https://vitejs.dev/config/ for more", None),
            ("Network: http://192.168.1.4:5173/", None),
        ];
        for (line, url) in cases {
            assert_eq!(local_url(line).as_deref(), url, "{line}");
        }
        let mut parser = OutputParser::new("/w".into());
        assert!(parser.feed("  ➜  Local:   http://localhost:5173/").url);
        assert!(!parser.feed("  ➜  Local:   http://localhost:5174/").url, "the first one stays");
        assert_eq!(parser.url.as_deref(), Some("http://localhost:5173/"));
    }

    fn issues(lines: &[&str]) -> Vec<String> {
        let mut parser = OutputParser::new("/app".into());
        for line in lines {
            parser.feed(line);
        }
        parser
            .issues
            .iter()
            .map(|i| format!("{:?} {}:{}:{} {}", i.severity, i.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(), i.line.unwrap_or(0), i.column.unwrap_or(0), i.message))
            .collect()
    }

    #[test]
    fn typescript_errors_in_both_formats() {
        assert_eq!(
            issues(&[
                "src/main.ts(12,5): error TS2322: Type 'string' is not assignable to type 'number'.",
                "src/counter.ts:3:7 - error TS2304: Cannot find name 'x'.",
                "/abs/src/a.ts(1,1): warning TS6133: 'y' is declared but its value is never read.",
                "Found 3 errors. Watching for file changes.",
            ]),
            [
                "Error /app/src/main.ts:12:5 Type 'string' is not assignable to type 'number'.",
                "Error /app/src/counter.ts:3:7 Cannot find name 'x'.",
                "Warning /abs/src/a.ts:1:1 'y' is declared but its value is never read.",
            ]
        );
    }

    #[test]
    fn esbuild_and_vite_errors_with_their_location_below() {
        assert_eq!(
            issues(&[
                "✘ [ERROR] Expected \";\" but found \"b\"",
                "",
                "    src/main.ts:3:7:",
                "      3 │ let a b",
                "        ╵        ^",
                "12:00:00 PM [vite] Internal server error: /app/src/App.vue:3:7: Unexpected token",
                "12:00:01 PM [vite] (client) Internal server error: [vue/compiler-sfc] Missing semicolon. (4:9)",
                "/app/src/Greeting.vue",
                "2  |  const n = 1",
                "  Plugin: vite:vue",
                "  File: /app/src/Greeting.vue:4:9",
                "[vite] Internal server error: Failed to resolve import \"./nope\" from \"src/main.ts\". Does the file exist?",
                "/app/src/x.ts:9:2: ERROR: Unterminated string literal",
            ]),
            [
                "Error /app/src/main.ts:3:7 Expected \";\" but found \"b\"",
                "Error /app/src/App.vue:3:7 Unexpected token",
                "Error /app/src/Greeting.vue:4:9 [vue/compiler-sfc] Missing semicolon. (4:9)",
                "Error /app/src/main.ts:0:0 Failed to resolve import \"./nope\" from \"src/main.ts\". Does the file exist?",
                "Error /app/src/x.ts:9:2 Unterminated string literal",
            ]
        );
    }

    #[test]
    fn eslint_and_compiler_formats() {
        assert_eq!(
            issues(&[
                "/app/src/main.ts",
                "  3:7   error    'x' is assigned a value but never used  no-unused-vars",
                "  10:1  warning  Unexpected console statement            no-console",
                "",
                "✖ 2 problems (1 error, 1 warning)",
                "src/b.js:2:4: 'y' is not defined. [Error/no-undef]",
                "src/c.ts:1:2: error: something broke",
                "/abs/d.swift:5:1: warning: deprecated",
            ]),
            [
                "Error /app/src/main.ts:3:7 'x' is assigned a value but never used",
                "Warning /app/src/main.ts:10:1 Unexpected console statement",
                "Error /app/src/b.js:2:4 'y' is not defined.",
                "Error /app/src/c.ts:1:2 something broke",
                "Warning /abs/d.swift:5:1 deprecated",
            ]
        );
        assert!(issues(&["  3:7  error  not after a file  rule", "INFO  ready in 300 ms"]).is_empty());
    }

    #[test]
    fn a_watcher_starting_over_clears_what_it_reported() {
        let mut parser = OutputParser::new("/app".into());
        parser.feed("src/main.ts(1,1): error TS1005: ';' expected.");
        parser.feed("src/main.ts(1,1): error TS1005: ';' expected.");
        assert_eq!(parser.issues.len(), 1, "repeated, not added twice");
        assert_eq!(parser.feed("[12:00:00 PM] File change detected. Starting incremental compilation..."), Fed { url: false, issues: true });
        assert!(parser.issues.is_empty());
        parser.feed("✘ [ERROR] Broken");
        parser.feed("12:00:02 PM [vite] (client) hmr update /src/App.vue");
        assert!(parser.issues.is_empty());
    }
}

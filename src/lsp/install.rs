//! Finding a language server's program, and installing the npm ones zcode
//! keeps itself.
//!
//! A server's program is, in order: `lsp.<id>.binary.path` from your
//! settings; the web app's own `node_modules/.bin` (or any up to the
//! project's folder); your login shell's `PATH`; zcode's copy in
//! `~/Library/Application Support/zcode/language-servers/<package>`,
//! installed with npm the first time it's needed unless
//! `auto_install_language_servers` is off. Each package's copy is installed
//! once at a time, however many servers (HTML, CSS, JSON, ESLint) share it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use anyhow::{Context as _, Result, bail};
use parking_lot::Mutex;

use super::servers::{Launch, ServerSpec};

/// A program to start and how.
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub path: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Where the program was looked for, and what to do when it isn't there.
#[derive(Clone, Debug, Default)]
pub struct Lookup {
    /// `lsp.<id>.binary`: a path and its arguments.
    pub binary: Option<(String, Option<Vec<String>>)>,
    /// The folder the server runs for, and the project's.
    pub root: PathBuf,
    pub project_root: PathBuf,
    pub auto_install: bool,
}

/// What finding a program came to.
pub enum Found {
    Program(Program),
    /// Not installed yet; `install` installs it.
    Install,
}

/// zcode's own copies of npm servers. `ZCODE_LANGUAGE_SERVERS_DIR` stands
/// in (the e2e tests share one copy between runs).
pub fn servers_dir() -> PathBuf {
    match std::env::var_os("ZCODE_LANGUAGE_SERVERS_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => crate::paths::support_dir().join("language-servers"),
    }
}

/// The folder zcode installs `package` in.
pub fn package_dir(package: &str) -> PathBuf {
    servers_dir().join(package.trim_start_matches('@').replace('/', "-"))
}

/// Looks for `spec`'s program. Blocking (it may run the login shell once).
pub fn find(spec: &ServerSpec, lookup: &Lookup) -> Result<Found> {
    let path = crate::acp::agents::login_path();
    // npm's servers are scripts run by `#!/usr/bin/env node`.
    let env = vec![("PATH".to_string(), path.to_string())];
    let args = |own: Option<&Vec<String>>| own.cloned().unwrap_or_else(|| spec.args.iter().map(|a| a.to_string()).collect());
    if let Some((binary, own_args)) = &lookup.binary {
        let program = crate::acp::agents::find_program(binary, path)
            .with_context(|| format!("“{binary}” (lsp.{}.binary.path) wasn’t found", spec.id))?;
        return Ok(Found::Program(Program { path: program, args: args(own_args.as_ref()), env }));
    }
    let program = |bin: PathBuf| Found::Program(Program { path: bin, args: args(None), env: env.clone() });
    match &spec.launch {
        Launch::Xcrun(tool) => Ok(program(crate::toolchain::find_tool(tool)?)),
        Launch::Project { bin } => match project_bin(bin, lookup) {
            Some(bin) => Ok(program(bin)),
            None => bail!("{bin} isn’t installed in this project’s node_modules (npm install)"),
        },
        Launch::Npm { package, bin, .. } => {
            if let Some(found) = project_bin(bin, lookup).or_else(|| crate::acp::agents::find_program(bin, path)) {
                return Ok(program(found));
            }
            let own = package_dir(package).join("node_modules/.bin").join(bin);
            if own.is_file() {
                return Ok(program(own));
            }
            if !lookup.auto_install {
                bail!("{bin} isn’t installed: npm install -g {package}, or turn on auto_install_language_servers");
            }
            Ok(Found::Install)
        }
    }
}

/// `bin` in a `node_modules/.bin` from the server's folder up to the project's.
fn project_bin(bin: &str, lookup: &Lookup) -> Option<PathBuf> {
    lookup
        .root
        .ancestors()
        .take_while(|dir| dir.starts_with(&lookup.project_root) || *dir == lookup.root)
        .map(|dir| dir.join("node_modules/.bin").join(bin))
        .find(|candidate| candidate.is_file())
}

fn install_locks() -> &'static Mutex<HashMap<PathBuf, Arc<Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(Default::default)
}

/// Installs `spec`'s package (and what goes with it) into zcode's folder
/// with npm, then finds its program there. Blocking; npm's output goes to
/// `log`.
pub fn install(spec: &ServerSpec, lookup: &Lookup, log: &Path) -> Result<Program> {
    let Launch::Npm { package, bin, with } = &spec.launch else {
        bail!("{} isn’t installed by zcode", spec.id);
    };
    let dir = package_dir(package);
    let lock = install_locks().lock().entry(dir.clone()).or_default().clone();
    let _installing = lock.lock();
    // Another server sharing the package may have installed it meanwhile.
    if !dir.join("node_modules/.bin").join(bin).is_file() {
        let path = crate::acp::agents::login_path();
        let npm = crate::acp::agents::find_program("npm", path)
            .context("npm wasn’t found on your PATH: install Node.js (nodejs.org or Homebrew) for the web language servers")?;
        std::fs::create_dir_all(&dir)?;
        if !dir.join("package.json").exists() {
            std::fs::write(dir.join("package.json"), "{ \"private\": true }\n")?;
        }
        if let Some(parent) = log.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let output = std::fs::File::create(log)?;
        let status = std::process::Command::new(npm)
            .args(["install", "--no-audit", "--no-fund", "--no-progress", "--loglevel=error", "--prefix"])
            .arg(&dir)
            .arg(format!("{package}@latest"))
            .args(with.iter())
            .env("PATH", path)
            .current_dir(&dir)
            .stdin(std::process::Stdio::null())
            .stdout(output.try_clone()?)
            .stderr(output)
            .status()
            .context("running npm install")?;
        if !status.success() {
            let tail = std::fs::read_to_string(log).unwrap_or_default();
            let last = tail.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
            bail!("npm install {package} failed ({status}){}", if last.is_empty() { String::new() } else { format!(": {last}") });
        }
    }
    match find(spec, &Lookup { auto_install: false, ..lookup.clone() })? {
        Found::Program(program) => Ok(program),
        Found::Install => bail!("npm installed {package} but not {bin}"),
    }
}

/// TypeScript's `lib` folder for a server that loads it itself: the web
/// app's own TypeScript, else the copy installed next to `spec`'s package.
pub fn typescript_lib(spec: &ServerSpec, root: &Path, project_root: &Path) -> Option<PathBuf> {
    let own = root
        .ancestors()
        .take_while(|dir| dir.starts_with(project_root) || *dir == root)
        .map(|dir| dir.join("node_modules/typescript/lib"))
        .find(|lib| lib.join("typescript.js").is_file());
    own.or_else(|| match &spec.launch {
        Launch::Npm { package, .. } => Some(package_dir(package).join("node_modules/typescript/lib")).filter(|lib| lib.is_dir()),
        _ => None,
    })
}

/// `@vue/language-server`'s folder (its TypeScript plugin, for vtsls): the
/// web app's own, else zcode's copy, installed if need be.
pub fn vue_language_server(root: &Path, project_root: &Path) -> Option<PathBuf> {
    root.ancestors()
        .take_while(|dir| dir.starts_with(project_root) || *dir == root)
        .map(|dir| dir.join("node_modules/@vue/language-server"))
        .chain([package_dir("@vue/language-server").join("node_modules/@vue/language-server")])
        .find(|dir| dir.join("package.json").is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_get_a_folder_each() {
        assert!(package_dir("@vtsls/language-server").ends_with("vtsls-language-server"));
        assert!(package_dir("vscode-langservers-extracted").ends_with("vscode-langservers-extracted"));
    }

    #[test]
    fn a_web_apps_own_server_comes_first() {
        let root = std::env::temp_dir().join(format!("zcode-install-{}", std::process::id()));
        let web = root.join("web");
        let bin = web.join("node_modules/.bin/biome");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "").unwrap();
        let lookup = Lookup { root: web.clone(), project_root: root.clone(), ..Default::default() };
        assert_eq!(project_bin("biome", &lookup), Some(bin));
        assert_eq!(project_bin("vtsls", &lookup), None);
        std::fs::remove_dir_all(&root).ok();
    }
}

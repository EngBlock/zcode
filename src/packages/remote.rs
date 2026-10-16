//! What Add Package Dependencies asks a repository before adding it: its
//! release tags and branches (`git ls-remote`), and the products its
//! manifest declares at the version a rule picks (a shallow fetch of that
//! version, then `swift package dump-package`). Blocking; run these in the
//! background.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::Value;

use super::rule::{Release, Rule, releases};

/// Turns what was typed in the search field into a repository location:
/// `owner/repo` and `github.com/owner/repo` become GitHub URLs; URLs,
/// `git@host:path` and absolute paths are kept.
pub fn normalize_url(input: &str) -> String {
    let s = input.trim().trim_end_matches('/');
    if s.contains("://") || s.starts_with('/') || s.starts_with('~') || (s.contains('@') && s.contains(':')) {
        return s.to_string();
    }
    if s.starts_with("github.com/") || s.starts_with("www.github.com/") {
        return format!("https://{}", s.trim_start_matches("www."));
    }
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() == 2 && parts.iter().all(|p| !p.is_empty() && !p.contains(char::is_whitespace)) {
        return format!("https://github.com/{s}");
    }
    s.to_string()
}

/// A repository's release tags (newest first), branches and default branch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Refs {
    pub releases: Vec<Release>,
    pub branches: Vec<String>,
    pub default_branch: Option<String>,
}

fn git() -> Command {
    let mut cmd = Command::new("/usr/bin/git");
    // Never wait on a credentials prompt nobody can see; give up on a
    // connection that stalls.
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/usr/bin/false")
        .args(["-c", "http.lowSpeedLimit=1", "-c", "http.lowSpeedTime=30"])
        .stdin(Stdio::null());
    cmd
}

fn run(mut cmd: Command, what: &str) -> Result<String> {
    let output = cmd.output().with_context(|| format!("couldn’t run {what}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr
            .lines()
            .map(|l| l.trim().trim_start_matches("fatal: ").trim_start_matches("error: "))
            .rfind(|l| !l.is_empty())
            .unwrap_or("failed")
            .to_string();
        bail!("{message}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Reads `git ls-remote --symref <url> HEAD refs/heads/* refs/tags/*`.
pub fn parse_ls_remote(output: &str) -> Refs {
    let mut tags = vec![];
    let mut refs = Refs::default();
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("ref: ") {
            // `ref: refs/heads/main\tHEAD`
            if let Some((target, "HEAD")) = rest.split_once('\t') {
                refs.default_branch = target.strip_prefix("refs/heads/").map(str::to_string);
            }
            continue;
        }
        let Some((_, name)) = line.split_once('\t') else { continue };
        if let Some(tag) = name.strip_prefix("refs/tags/") {
            tags.push(tag.trim_end_matches("^{}").to_string());
        } else if let Some(branch) = name.strip_prefix("refs/heads/") {
            refs.branches.push(branch.to_string());
        }
    }
    tags.dedup();
    refs.releases = releases(tags);
    refs.branches.sort_by_key(|b| (refs.default_branch.as_deref() != Some(b.as_str()), b.to_lowercase()));
    refs
}

/// Lists a repository's tags and branches.
pub fn list_refs(url: &str) -> Result<Refs> {
    let mut cmd = git();
    cmd.args(["ls-remote", "--symref", url, "HEAD", "refs/heads/*", "refs/tags/*"]);
    let output = run(cmd, "git ls-remote")?;
    Ok(parse_ls_remote(&output))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProductKind {
    Library,
    Executable,
    Plugin,
    Other,
}

impl ProductKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Library => "Library",
            Self::Executable => "Executable",
            Self::Plugin => "Plugin",
            Self::Other => "Product",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Product {
    pub name: String,
    pub kind: ProductKind,
}

/// The parts of a manifest Add Package uses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub products: Vec<Product>,
    /// Targets a package product can be added to, with their type
    /// (`regular`, `executable`, `test`).
    pub targets: Vec<(String, String)>,
}

/// Reads the JSON `swift package dump-package` prints.
pub fn parse_dump(json: &str) -> Result<Manifest> {
    let root: Value = serde_json::from_str(json).context("reading the package manifest")?;
    let name = root.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
    let products = root
        .get("products")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|p| {
                    let name = p.get("name")?.as_str()?.to_string();
                    let ty = p.get("type")?;
                    let kind = if ty.get("library").is_some() {
                        ProductKind::Library
                    } else if ty.get("executable").is_some() {
                        ProductKind::Executable
                    } else if ty.get("plugin").is_some() {
                        ProductKind::Plugin
                    } else {
                        ProductKind::Other
                    };
                    Some(Product { name, kind })
                })
                .collect()
        })
        .unwrap_or_default();
    let targets = root
        .get("targets")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|t| Some((t.get("name")?.as_str()?.to_string(), t.get("type")?.as_str()?.to_string())))
                .filter(|(_, ty)| !matches!(ty.as_str(), "plugin" | "binary" | "system" | "macro"))
                .collect()
        })
        .unwrap_or_default();
    Ok(Manifest { name, products, targets })
}

/// `swift package dump-package` for the package in `dir`.
pub fn dump_package(dir: &Path) -> Result<Manifest> {
    let mut cmd = Command::new("/usr/bin/swift");
    cmd.args(["package", "dump-package", "--package-path"]).arg(dir).stdin(Stdio::null());
    let output = run(cmd, "swift package dump-package")?;
    parse_dump(&output)
}

/// What a rule resolves to for fetching: a tag or branch name, or a commit.
pub fn checkout_ref(rule: &Rule, releases: &[Release]) -> Result<String> {
    match rule {
        Rule::Branch(branch) => Ok(branch.clone()),
        Rule::Commit(sha) => Ok(sha.clone()),
        _ => rule
            .pick(releases)
            .map(|r| r.tag.clone())
            .ok_or_else(|| anyhow!("No version of the package matches {}.", rule.summary())),
    }
}

/// Fetches `reference` of `url` into a fresh folder under `scratch` and
/// reads its manifest. The folder is removed afterwards.
pub fn fetch_manifest(url: &str, reference: &str, is_commit: bool, scratch: &Path) -> Result<Manifest> {
    std::fs::create_dir_all(scratch)?;
    let dir: PathBuf = scratch.join(format!("checkout-{}-{}", std::process::id(), unique()));
    let result = (|| {
        if is_commit {
            let mut init = git();
            init.args(["init", "-q"]).arg(&dir);
            run(init, "git init")?;
            let mut fetch = git();
            fetch.arg("-C").arg(&dir).args(["fetch", "-q", "--depth", "1", url, reference]);
            if run(fetch, "git fetch").is_err() {
                // Not every server serves a commit by hash; fetch it all.
                let mut fetch = git();
                fetch.arg("-C").arg(&dir).args(["fetch", "-q", url]);
                run(fetch, "git fetch")?;
                let mut checkout = git();
                checkout.arg("-C").arg(&dir).args(["checkout", "-q", reference]);
                run(checkout, "git checkout")?;
            } else {
                let mut checkout = git();
                checkout.arg("-C").arg(&dir).args(["checkout", "-q", "FETCH_HEAD"]);
                run(checkout, "git checkout")?;
            }
        } else {
            let mut clone = git();
            clone.args(["clone", "-q", "--depth", "1", "--branch", reference, url]).arg(&dir);
            run(clone, "git clone")?;
        }
        if !dir.join("Package.swift").exists() {
            bail!("{url} has no Package.swift at {reference}; it isn’t a Swift package.");
        }
        dump_package(&dir)
    })();
    std::fs::remove_dir_all(&dir).ok();
    result
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_search_text() {
        assert_eq!(normalize_url("apple/swift-collections"), "https://github.com/apple/swift-collections");
        assert_eq!(normalize_url("github.com/apple/swift-collections/"), "https://github.com/apple/swift-collections");
        assert_eq!(normalize_url(" https://github.com/apple/swift-collections.git "), "https://github.com/apple/swift-collections.git");
        assert_eq!(normalize_url("git@github.com:apple/swift-collections.git"), "git@github.com:apple/swift-collections.git");
        assert_eq!(normalize_url("file:///tmp/Tally.git"), "file:///tmp/Tally.git");
        assert_eq!(normalize_url("/tmp/Tally.git"), "/tmp/Tally.git");
        assert_eq!(normalize_url("collections"), "collections");
    }

    #[test]
    fn reads_ls_remote() {
        let out = "ref: refs/heads/main\tHEAD\n\
                   aaaa\tHEAD\n\
                   bbbb\trefs/heads/develop\n\
                   aaaa\trefs/heads/main\n\
                   cccc\trefs/tags/1.0.0\n\
                   dddd\trefs/tags/1.1.0\n\
                   eeee\trefs/tags/1.1.0^{}\n\
                   ffff\trefs/tags/nightly\n";
        let refs = parse_ls_remote(out);
        assert_eq!(refs.default_branch.as_deref(), Some("main"));
        assert_eq!(refs.branches, ["main", "develop"]);
        assert_eq!(refs.releases.iter().map(|r| r.tag.as_str()).collect::<Vec<_>>(), ["1.1.0", "1.0.0"]);
    }

    #[test]
    fn reads_products() {
        let json = r#"{"name":"Tally","products":[
            {"name":"Tally","targets":["Tally"],"type":{"library":["automatic"]}},
            {"name":"tally-cli","targets":["CLI"],"type":{"executable":null}},
            {"name":"Lint","targets":["Lint"],"type":{"plugin":null}}],
            "targets":[{"name":"Tally","type":"regular"},{"name":"CLI","type":"executable"},{"name":"TallyTests","type":"test"},{"name":"Lint","type":"plugin"}]}"#;
        let manifest = parse_dump(json).unwrap();
        assert_eq!(manifest.name, "Tally");
        let kinds: Vec<(&str, ProductKind)> = manifest.products.iter().map(|p| (p.name.as_str(), p.kind)).collect();
        assert_eq!(kinds, [("Tally", ProductKind::Library), ("tally-cli", ProductKind::Executable), ("Lint", ProductKind::Plugin)]);
        assert_eq!(manifest.targets.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["Tally", "CLI", "TallyTests"]);
    }

    #[test]
    fn picks_what_to_fetch() {
        let list = releases(["1.0.0", "1.2.0", "2.0.0"].map(String::from));
        assert_eq!(checkout_ref(&Rule::UpToNextMajor("1.0.0".into()), &list).unwrap(), "1.2.0");
        assert_eq!(checkout_ref(&Rule::Branch("develop".into()), &list).unwrap(), "develop");
        assert!(checkout_ref(&Rule::Exact("3.0.0".into()), &list).is_err());
    }
}

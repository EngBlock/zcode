//! Git, through the `git` command line (`/usr/bin/git`, which Xcode's
//! command line tools install): status, the index and `HEAD` versions of
//! files, staging (whole files, or hunks written straight into the index),
//! commits, blame, branches, and fetch, pull and push.
//!
//! Every function here blocks; the workspace calls them on background
//! threads. Nothing ever waits on a prompt: credentials come from git's
//! helpers (the keychain, an SSH agent) or the operation fails with git's
//! own message.

pub mod blame;
pub mod diff;

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};

/// git with the repository at `root` as its working directory.
fn git(root: &Path) -> Command {
    let mut cmd = Command::new("/usr/bin/git");
    cmd.current_dir(root)
        // Never wait on a credentials prompt nobody can see, nor on an editor.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/usr/bin/false")
        .env("GIT_EDITOR", "true")
        .env("GIT_MERGE_AUTOEDIT", "no")
        // Paths as they are, not quoted octal escapes.
        .args(["-c", "core.quotepath=false"])
        .stdin(Stdio::null());
    if std::env::var_os("GIT_SSH_COMMAND").is_none() && std::env::var_os("GIT_SSH").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    cmd
}

/// Runs `cmd`, feeding it `input`; its stdout, or git's message when it fails.
fn run_bytes(mut cmd: Command, input: Option<&[u8]>) -> Result<Vec<u8>> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    if input.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd.spawn().context("couldn’t run git")?;
    if let Some(input) = input {
        // A writer thread, so a large output can't deadlock against the input.
        let mut stdin = child.stdin.take().context("no stdin")?;
        let input = input.to_vec();
        std::thread::spawn(move || stdin.write_all(&input).ok());
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!("{}", error_message(&String::from_utf8_lossy(&output.stderr), &String::from_utf8_lossy(&output.stdout)));
    }
    Ok(output.stdout)
}

fn run(cmd: Command) -> Result<String> {
    run_bytes(cmd, None).map(|out| String::from_utf8_lossy(&out).into_owned())
}

/// What went wrong, from git's output: its `fatal:` or `error:` line, with
/// the reason a push was rejected, or the files a line ending in `:` lists
/// under it (the local changes a checkout would overwrite).
pub fn error_message(stderr: &str, stdout: &str) -> String {
    let raw: Vec<&str> = stderr.lines().chain(stdout.lines()).filter(|l| !l.trim().is_empty()).collect();
    let lines: Vec<&str> = raw.iter().map(|l| l.trim()).collect();
    let rejected = lines.iter().find(|l| l.starts_with("! [rejected]") || l.starts_with("! [remote rejected]"));
    let at = lines
        .iter()
        .position(|l| l.starts_with("fatal: ") || l.starts_with("error: "))
        .or(lines.iter().position(|l| !l.starts_with("hint: ")));
    let main = at.map(|at| lines[at].trim_start_matches("fatal: ").trim_start_matches("error: ")).unwrap_or("git failed");
    // git indents the files it lists with a tab.
    let listed: Vec<&str> = match at {
        Some(at) if main.ends_with(':') => {
            raw[at + 1..].iter().take_while(|l| l.starts_with(['\t', ' '])).map(|l| l.trim()).collect()
        }
        _ => vec![],
    };
    const LISTED: usize = 5;
    let main = match listed.len() {
        0 => main.to_string(),
        n if n <= LISTED => format!("{main} {}", listed.join(", ")),
        n => format!("{main} {} and {} more", listed[..LISTED].join(", "), n - LISTED),
    };
    match rejected {
        Some(rejected) => format!("{main} ({})", rejected.trim_start_matches("! ")),
        None => main,
    }
}

/// The top of the work tree `path` is in, if it is in one.
pub fn discover(path: &Path) -> Option<PathBuf> {
    let dir = if path.is_dir() { path } else { path.parent()? };
    let mut cmd = git(dir);
    cmd.args(["rev-parse", "--show-toplevel"]);
    let top = run(cmd).ok()?;
    let top = PathBuf::from(top.trim());
    Some(top.canonicalize().unwrap_or(top))
}

/// The repository's git directory (`.git`, or a worktree's).
pub fn git_dir(root: &Path) -> Option<PathBuf> {
    let mut cmd = git(root);
    cmd.args(["rev-parse", "--absolute-git-dir"]);
    run(cmd).ok().map(|dir| PathBuf::from(dir.trim()))
}

/// `path` relative to the work tree, with `/` separators, as git names it.
pub fn relative(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root).ok().map(|rel| rel.to_string_lossy().into_owned())
}

/// The user's `user.email` (to call their own commits "You").
pub fn user_email(root: &Path) -> Option<String> {
    let mut cmd = git(root);
    cmd.args(["config", "user.email"]);
    run(cmd).ok().map(|e| e.trim().to_string()).filter(|e| !e.is_empty())
}

// ---------------------------------------------------------------------------
// Status

/// A file's state in the index or the work tree, as `git status` letters it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Unmodified,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
}

impl Change {
    fn from_letter(letter: char) -> Self {
        match letter {
            'M' => Self::Modified,
            'A' => Self::Added,
            'D' => Self::Deleted,
            'R' => Self::Renamed,
            'C' => Self::Copied,
            'T' => Self::TypeChanged,
            'U' => Self::Conflicted,
            _ => Self::Unmodified,
        }
    }

    /// Xcode's letter in the Source Control navigator.
    pub fn letter(self) -> &'static str {
        match self {
            Self::Unmodified => "",
            Self::Modified | Self::TypeChanged => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Copied => "C",
            Self::Untracked => "?",
            Self::Conflicted => "!",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileStatus {
    /// Absolute.
    pub path: PathBuf,
    /// Staged: the index against `HEAD`.
    pub index: Change,
    /// Unstaged: the work tree against the index.
    pub worktree: Change,
    /// A rename's old path.
    pub orig: Option<PathBuf>,
}

impl FileStatus {
    pub fn is_staged(&self) -> bool {
        !matches!(self.index, Change::Unmodified | Change::Untracked | Change::Conflicted)
    }

    pub fn is_unstaged(&self) -> bool {
        self.worktree != Change::Unmodified
    }

    /// The most significant letter: the work tree's change, else the index's.
    pub fn letter(&self) -> &'static str {
        match self.worktree {
            Change::Unmodified => self.index.letter(),
            change => change.letter(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// The checked-out branch; `None` when `HEAD` is detached.
    pub branch: Option<String>,
    /// `HEAD`'s commit; `None` before the first commit.
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<FileStatus>,
}

impl Status {
    pub fn file(&self, path: &Path) -> Option<&FileStatus> {
        self.files.iter().find(|f| f.path == path)
    }
}

pub fn status(root: &Path) -> Result<Status> {
    let mut cmd = git(root);
    // No optional locks: a status running in the background must not make a
    // commit in a terminal fail on `index.lock`.
    cmd.args(["--no-optional-locks", "status", "--porcelain=v2", "--branch", "-z", "-uall"]);
    Ok(parse_status(root, &run(cmd)?))
}

/// Reads `git status --porcelain=v2 --branch -z`.
pub fn parse_status(root: &Path, out: &str) -> Status {
    let mut status = Status::default();
    let mut fields = out.split('\0').filter(|f| !f.is_empty()).peekable();
    while let Some(entry) = fields.next() {
        if let Some(header) = entry.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value != "(initial)" => status.head = Some(value.to_string()),
                "branch.head" if value != "(detached)" => status.branch = Some(value.to_string()),
                "branch.upstream" => status.upstream = Some(value.to_string()),
                "branch.ab" => {
                    for part in value.split_whitespace() {
                        if let Some(n) = part.strip_prefix('+') {
                            status.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            status.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let kind = entry.chars().next().unwrap_or(' ');
        let xy = |field: &str| {
            let mut letters = field.chars();
            (Change::from_letter(letters.next().unwrap_or('.')), Change::from_letter(letters.next().unwrap_or('.')))
        };
        match kind {
            // 1 XY sub mH mI mW hH hI path
            '1' => {
                let parts: Vec<&str> = entry.splitn(9, ' ').collect();
                if let (Some(code), Some(path)) = (parts.get(1), parts.get(8)) {
                    let (index, worktree) = xy(code);
                    status.files.push(FileStatus { path: root.join(path), index, worktree, orig: None });
                }
            }
            // 2 XY sub mH mI mW hH hI Xscore path, then the original path
            '2' => {
                let parts: Vec<&str> = entry.splitn(10, ' ').collect();
                let orig = fields.next().map(|p| root.join(p));
                if let (Some(code), Some(path)) = (parts.get(1), parts.get(9)) {
                    let (index, worktree) = xy(code);
                    status.files.push(FileStatus { path: root.join(path), index, worktree, orig });
                }
            }
            // u XY sub m1 m2 m3 mW h1 h2 h3 path
            'u' => {
                if let Some(path) = entry.splitn(11, ' ').nth(10) {
                    status.files.push(FileStatus {
                        path: root.join(path),
                        index: Change::Conflicted,
                        worktree: Change::Conflicted,
                        orig: None,
                    });
                }
            }
            '?' => status.files.push(FileStatus {
                path: root.join(&entry[2..]),
                index: Change::Untracked,
                worktree: Change::Untracked,
                orig: None,
            }),
            _ => {}
        }
    }
    status
}

// ---------------------------------------------------------------------------
// The index and HEAD versions of files

/// A file's versions to compare the editor's text with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bases {
    /// Blob ids, to tell when a version changed without reading it.
    pub head_id: Option<String>,
    pub index_id: Option<String>,
    /// The texts; `None` when the file isn't there or isn't UTF-8 text.
    pub head: Option<String>,
    pub index: Option<String>,
    /// The index entry's mode (`100644`, `100755`).
    pub mode: Option<String>,
}

/// The blob ids of `rels` in the index (stage 0) and in `HEAD`.
pub fn blob_ids(root: &Path, rels: &[String]) -> (HashMap<String, (String, String)>, HashMap<String, String>) {
    let mut index = HashMap::new();
    let mut head = HashMap::new();
    if rels.is_empty() {
        return (index, head);
    }
    let mut cmd = git(root);
    cmd.args(["ls-files", "-s", "-z", "--"]).args(rels);
    let listed = run(cmd);
    // One path git refuses (inside a nested repository) fails the batch:
    // ask about each one instead.
    if listed.is_err() && rels.len() > 1 {
        for rel in rels {
            let (one_index, one_head) = blob_ids(root, std::slice::from_ref(rel));
            index.extend(one_index);
            head.extend(one_head);
        }
        return (index, head);
    }
    // "<mode> <oid> <stage>\t<path>"
    for entry in listed.unwrap_or_default().split('\0') {
        let Some((meta, path)) = entry.split_once('\t') else { continue };
        let parts: Vec<&str> = meta.split(' ').collect();
        if let [mode, oid, "0"] = parts[..] {
            index.insert(path.to_string(), (oid.to_string(), mode.to_string()));
        }
    }
    let mut cmd = git(root);
    cmd.args(["ls-tree", "-z", "HEAD", "--"]).args(rels);
    // "<mode> blob <oid>\t<path>"
    for entry in run(cmd).unwrap_or_default().split('\0') {
        let Some((meta, path)) = entry.split_once('\t') else { continue };
        let parts: Vec<&str> = meta.split(' ').collect();
        if let [_, "blob", oid] = parts[..] {
            head.insert(path.to_string(), oid.to_string());
        }
    }
    // A staged rename's HEAD version is its old path's.
    if rels.iter().any(|rel| index.contains_key(rel) && !head.contains_key(rel)) {
        for (new, old_id) in staged_renames(root) {
            if rels.contains(&new) && index.contains_key(&new) && !head.contains_key(&new) {
                head.insert(new, old_id);
            }
        }
    }
    (index, head)
}

/// The index's renames of `HEAD`'s files: each new path, with the blob id
/// of its old path in `HEAD`.
fn staged_renames(root: &Path) -> Vec<(String, String)> {
    let mut cmd = git(root);
    // No pathspec: it would hide the old paths from rename detection.
    cmd.args(["diff-index", "--cached", "-M", "-z", "HEAD"]);
    let Ok(out) = run(cmd) else { return vec![] };
    // ":<old mode> <new mode> <old oid> <new oid> R<score>\0<old path>\0<new path>\0",
    // other changes with one path.
    let mut renames = vec![];
    let mut fields = out.split('\0');
    while let Some(meta) = fields.next() {
        let parts: Vec<&str> = meta.split(' ').collect();
        let [_, _, old_id, _, change] = parts[..] else { continue };
        // The (old) path.
        if fields.next().is_none() {
            break;
        }
        if change.starts_with('R') {
            let Some(new) = fields.next() else { break };
            renames.push((new.to_string(), old_id.to_string()));
        }
    }
    renames
}

/// Reads blobs by id (`git cat-file --batch`); text blobs only.
pub fn read_blobs(root: &Path, ids: &[String]) -> HashMap<String, String> {
    let mut blobs = HashMap::new();
    if ids.is_empty() {
        return blobs;
    }
    let mut cmd = git(root);
    cmd.args(["cat-file", "--batch"]);
    let input = ids.join("\n") + "\n";
    let Ok(out) = run_bytes(cmd, Some(input.as_bytes())) else { return blobs };
    // "<oid> blob <size>\n<contents>\n" per id ("<id> missing\n" otherwise).
    let mut at = 0;
    while at < out.len() {
        let Some(newline) = out[at..].iter().position(|&b| b == b'\n') else { break };
        let header = String::from_utf8_lossy(&out[at..at + newline]).into_owned();
        at += newline + 1;
        let parts: Vec<&str> = header.split(' ').collect();
        let [oid, _, size] = parts[..] else { continue };
        let Ok(size) = size.parse::<usize>() else { continue };
        let Some(bytes) = out.get(at..at + size) else { break };
        at += size + 1;
        if let Ok(text) = std::str::from_utf8(bytes)
            && !bytes.contains(&0)
        {
            blobs.insert(oid.to_string(), text.to_string());
        }
    }
    blobs
}

/// The index and `HEAD` versions of `rels`, reusing `known` texts by blob id.
pub fn bases(root: &Path, rels: &[String], known: &HashMap<String, String>) -> Vec<Bases> {
    let (index, head) = blob_ids(root, rels);
    let mut wanted: Vec<String> = vec![];
    for rel in rels {
        for id in [index.get(rel).map(|(id, _)| id), head.get(rel)].into_iter().flatten() {
            if !known.contains_key(id) && !wanted.contains(id) {
                wanted.push(id.clone());
            }
        }
    }
    let read = read_blobs(root, &wanted);
    let text = |id: Option<&String>| id.and_then(|id| known.get(id).or_else(|| read.get(id)).cloned());
    rels.iter()
        .map(|rel| {
            let (index_id, mode) = index.get(rel).cloned().unzip();
            let head_id = head.get(rel).cloned();
            Bases { head: text(head_id.as_ref()), index: text(index_id.as_ref()), head_id, index_id, mode }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Staging

pub fn stage_files(root: &Path, rels: &[String]) -> Result<()> {
    let mut cmd = git(root);
    cmd.args(["add", "-A", "--"]).args(rels);
    run(cmd).map(drop)
}

pub fn unstage_files(root: &Path, rels: &[String]) -> Result<()> {
    let mut cmd = git(root);
    if has_head(root) {
        cmd.args(["reset", "-q", "HEAD", "--"]).args(rels);
    } else {
        cmd.args(["rm", "-q", "-r", "--cached", "--"]).args(rels);
    }
    run(cmd).map(drop)
}

fn has_head(root: &Path) -> bool {
    let mut cmd = git(root);
    cmd.args(["rev-parse", "-q", "--verify", "HEAD"]);
    run(cmd).is_ok()
}

/// Makes `text` the index's version of `rel` (`None` removes it from the
/// index): how a hunk is staged or unstaged without a patch. `mode` is the
/// entry's mode, `100644` for a new one.
pub fn write_index(root: &Path, rel: &str, text: Option<&str>, mode: Option<&str>) -> Result<()> {
    let Some(text) = text else {
        let mut cmd = git(root);
        cmd.args(["update-index", "--force-remove", "--", rel]);
        return run(cmd).map(drop);
    };
    let mut cmd = git(root);
    cmd.args(["hash-object", "-w", "--stdin", "--path", rel]);
    let oid = String::from_utf8_lossy(&run_bytes(cmd, Some(text.as_bytes()))?).trim().to_string();
    let mut cmd = git(root);
    let info = format!("{},{oid},{rel}", mode.unwrap_or("100644"));
    cmd.args(["update-index", "--add", "--cacheinfo", &info]);
    run(cmd).map(drop)
}

/// Discard Changes: tracked files back to `HEAD`, in the index and the work
/// tree (files added since `HEAD` leave the index; the caller trashes them).
pub fn discard_files(root: &Path, rels: &[String]) -> Result<()> {
    if rels.is_empty() {
        return Ok(());
    }
    if !has_head(root) {
        return unstage_files(root, rels);
    }
    let (_, head) = blob_ids(root, rels);
    let (tracked, added): (Vec<String>, Vec<String>) = rels.iter().cloned().partition(|rel| head.contains_key(rel));
    if !tracked.is_empty() {
        let mut cmd = git(root);
        cmd.args(["checkout", "-q", "HEAD", "--"]).args(&tracked);
        run(cmd)?;
    }
    if !added.is_empty() {
        let mut cmd = git(root);
        cmd.args(["rm", "-q", "-r", "--cached", "--ignore-unmatch", "--"]).args(&added);
        run(cmd)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Commits

/// Commits what is staged (`amend`: replaces `HEAD`); returns the new
/// commit's short id.
pub fn commit(root: &Path, message: &str, amend: bool) -> Result<String> {
    if message.trim().is_empty() {
        bail!("Enter a commit message.");
    }
    let mut cmd = git(root);
    cmd.args(["commit", "-q", "--cleanup=strip", "-F", "-"]);
    if amend {
        cmd.arg("--amend");
    }
    run_bytes(cmd, Some(message.as_bytes()))?;
    let mut cmd = git(root);
    cmd.args(["rev-parse", "--short", "HEAD"]);
    Ok(run(cmd)?.trim().to_string())
}

/// `HEAD`'s message, for Amend.
pub fn last_message(root: &Path) -> Option<String> {
    let mut cmd = git(root);
    cmd.args(["log", "-1", "--format=%B"]);
    run(cmd).ok().map(|m| m.trim_end().to_string())
}

/// A commit's full message (the Authors popover).
pub fn commit_message(root: &Path, sha: &str) -> Option<String> {
    let mut cmd = git(root);
    cmd.args(["log", "-1", "--format=%B", sha]);
    run(cmd).ok().map(|m| m.trim_end().to_string())
}

/// Blame for `rel` as the editor has it (`contents`, which may have
/// unsaved edits: their lines are "Not Committed Yet").
pub fn blame(root: &Path, rel: &str, contents: &str) -> Result<blame::Blame> {
    let mut cmd = git(root);
    cmd.args(["blame", "--porcelain", "--contents", "-", "--", rel]);
    let out = run_bytes(cmd, Some(contents.as_bytes()))?;
    Ok(blame::parse_porcelain(&String::from_utf8_lossy(&out)))
}

// ---------------------------------------------------------------------------
// Branches and remotes

#[derive(Clone, Debug, PartialEq)]
pub struct Branch {
    /// `main`, or `origin/main` for a remote branch.
    pub name: String,
    pub remote: bool,
    pub current: bool,
    pub upstream: Option<String>,
}

pub fn branches(root: &Path) -> Result<Vec<Branch>> {
    let mut cmd = git(root);
    cmd.args([
        "for-each-ref",
        "--format=%(refname)%00%(HEAD)%00%(upstream:short)",
        "--sort=refname",
        "refs/heads",
        "refs/remotes",
    ]);
    Ok(parse_branches(&run(cmd)?))
}

pub fn parse_branches(out: &str) -> Vec<Branch> {
    out.lines()
        .filter_map(|line| {
            let mut parts = line.split('\0');
            let refname = parts.next()?;
            let current = parts.next() == Some("*");
            let upstream = parts.next().filter(|u| !u.is_empty()).map(str::to_string);
            let (name, remote) = match refname.strip_prefix("refs/heads/") {
                Some(name) => (name, false),
                None => (refname.strip_prefix("refs/remotes/")?, true),
            };
            // `origin/HEAD` points at a branch listed anyway.
            if remote && name.ends_with("/HEAD") {
                return None;
            }
            Some(Branch { name: name.to_string(), remote, current, upstream })
        })
        .collect()
}

/// Checks out a branch; a remote one gets a local branch tracking it.
pub fn switch_branch(root: &Path, branch: &Branch) -> Result<()> {
    let mut cmd = git(root);
    let local = branch.name.split_once('/').map(|(_, name)| name).unwrap_or(&branch.name);
    if branch.remote && !branches(root)?.iter().any(|b| !b.remote && b.name == local) {
        cmd.args(["switch", "-q", "--track", &branch.name]);
    } else {
        cmd.args(["switch", "-q", if branch.remote { local } else { &branch.name }]);
    }
    run(cmd).map(drop)
}

/// New Branch: created from `HEAD` and checked out.
pub fn create_branch(root: &Path, name: &str) -> Result<()> {
    let mut cmd = git(root);
    cmd.args(["check-ref-format", "--branch", name]);
    if run(cmd).is_err() {
        bail!("“{name}” isn’t a valid branch name.");
    }
    let mut cmd = git(root);
    cmd.args(["switch", "-q", "-c", name]);
    run(cmd).map(drop)
}

pub fn remotes(root: &Path) -> Vec<String> {
    let mut cmd = git(root);
    cmd.arg("remote");
    run(cmd).map(|out| out.lines().map(str::to_string).collect()).unwrap_or_default()
}

/// Fetch Changes: every remote, pruning deleted branches.
pub fn fetch(root: &Path) -> Result<()> {
    let mut cmd = git(root);
    cmd.args(["fetch", "-q", "--all", "--prune"]);
    run(cmd).map(drop)
}

/// Pull…: merges (or rebases onto) `remote_branch` (`origin/main`).
pub fn pull(root: &Path, remote_branch: &str, rebase: bool) -> Result<()> {
    let (remote, branch) = remote_branch.split_once('/').context("choose a remote branch")?;
    let mut cmd = git(root);
    cmd.args(["pull", "-q", if rebase { "--rebase" } else { "--no-rebase" }, "--no-edit", remote, branch]);
    run(cmd).map(drop)
}

/// Push…: the current branch to `remote_branch` (`origin/main`), which
/// becomes its upstream; `tags` pushes every tag too.
pub fn push(root: &Path, local: &str, remote_branch: &str, tags: bool) -> Result<()> {
    let (remote, branch) = remote_branch.split_once('/').context("choose a remote branch")?;
    let mut cmd = git(root);
    cmd.args(["push", "-q", "--set-upstream", remote, &format!("{local}:refs/heads/{branch}")]);
    if tags {
        cmd.arg("--tags");
    }
    run(cmd).map(drop)
}

#[cfg(test)]
pub(crate) mod test_repo {
    //! Temporary repositories for tests, with git's global and system
    //! configuration out of the way.

    use std::path::{Path, PathBuf};
    use std::process::Command;

    pub struct TestRepo {
        pub root: PathBuf,
    }

    impl TestRepo {
        pub fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("zcode-git-{name}-{}-{}", std::process::id(), unique()));
            std::fs::create_dir_all(&dir).unwrap();
            let root = dir.canonicalize().unwrap();
            let repo = Self { root };
            repo.git(&["init", "-q", "-b", "main"]);
            repo.git(&["config", "user.name", "Test Author"]);
            repo.git(&["config", "user.email", "test@example.com"]);
            repo.git(&["config", "commit.gpgsign", "false"]);
            repo
        }

        pub fn bare(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("zcode-git-{name}-{}-{}.git", std::process::id(), unique()));
            std::fs::create_dir_all(&dir).unwrap();
            let repo = Self { root: dir.canonicalize().unwrap() };
            repo.git(&["init", "-q", "--bare", "-b", "main"]);
            repo
        }

        pub fn git(&self, args: &[&str]) -> String {
            let out = Command::new("/usr/bin/git")
                .current_dir(&self.root)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).into_owned()
        }

        pub fn write(&self, rel: &str, text: &str) -> PathBuf {
            let path = self.root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        }

        pub fn commit_all(&self, message: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", message]);
        }

        pub fn path(&self) -> &Path {
            &self.root
        }
    }

    impl Drop for TestRepo {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    fn unique() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        NEXT.fetch_add(1, Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::test_repo::TestRepo;
    use super::*;

    #[test]
    fn reads_porcelain_v2_status() {
        let root = Path::new("/r");
        let out = "# branch.oid 1234\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
                   1 .M N... 100644 100644 100644 aaa aaa Sources/A.swift\0\
                   1 A. N... 000000 100644 100644 000 bbb New File.swift\0\
                   2 R. N... 100644 100644 100644 ccc ccc R100 B.swift\0Old.swift\0\
                   u UU N... 100644 100644 100644 100644 a b c Conflict.swift\0\
                   ? notes.txt\0";
        let status = parse_status(root, out);
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.head.as_deref(), Some("1234"));
        assert_eq!((status.upstream.as_deref(), status.ahead, status.behind), (Some("origin/main"), 2, 1));
        let summary: Vec<(String, &str, bool, bool)> = status
            .files
            .iter()
            .map(|f| (f.path.strip_prefix(root).unwrap().display().to_string(), f.letter(), f.is_staged(), f.is_unstaged()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("Sources/A.swift".into(), "M", false, true),
                ("New File.swift".into(), "A", true, false),
                ("B.swift".into(), "R", true, false),
                ("Conflict.swift".into(), "!", false, true),
                ("notes.txt".into(), "?", false, true),
            ]
        );
        assert_eq!(status.files[2].orig.as_deref(), Some(Path::new("/r/Old.swift")));
        // Before the first commit, on a detached HEAD.
        let status = parse_status(root, "# branch.oid (initial)\0# branch.head (detached)\0");
        assert_eq!((status.head, status.branch), (None, None));
    }

    #[test]
    fn explains_failures_with_gits_own_words() {
        let stderr = "To /tmp/origin.git\n ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs to '/tmp/origin.git'\nhint: Updates were rejected\n";
        assert_eq!(error_message(stderr, ""), "failed to push some refs to '/tmp/origin.git' ([rejected]        main -> main (fetch first))");
        assert_eq!(error_message("fatal: not a git repository\n", ""), "not a git repository");
        assert_eq!(error_message("", "nothing to commit, working tree clean\n"), "nothing to commit, working tree clean");
        // The files a checkout would overwrite, as git lists them.
        let stderr = "error: Your local changes to the following files would be overwritten by checkout:\n\tSources/A.swift\n\tB.swift\n\
                      Please commit your changes or stash them before you switch branches.\nAborting\n";
        assert_eq!(
            error_message(stderr, ""),
            "Your local changes to the following files would be overwritten by checkout: Sources/A.swift, B.swift"
        );
        let many: String = (0..7).map(|i| format!("\tf{i}.txt\n")).collect();
        let stderr = format!("error: The following untracked working tree files would be overwritten by checkout:\n{many}Aborting\n");
        assert_eq!(
            error_message(&stderr, ""),
            "The following untracked working tree files would be overwritten by checkout: f0.txt, f1.txt, f2.txt, f3.txt, f4.txt and 2 more"
        );
    }

    #[test]
    fn status_bases_and_hunk_staging_in_a_real_repository() {
        let repo = TestRepo::new("status");
        repo.write("a.txt", "one\ntwo\nthree\n");
        repo.write("dir/b.txt", "bee\n");
        repo.commit_all("first");
        assert_eq!(discover(&repo.root.join("dir")), Some(repo.root.clone()));
        repo.write("a.txt", "one\n2\nthree\nfour\n");
        repo.write("new.txt", "new\n");
        let status = status(repo.path()).unwrap();
        assert_eq!(status.branch.as_deref(), Some("main"));
        let letters: Vec<(&str, &str)> =
            status.files.iter().map(|f| (f.path.file_name().unwrap().to_str().unwrap(), f.letter())).collect();
        assert_eq!(letters, vec![("a.txt", "M"), ("new.txt", "?")]);

        let rels = vec!["a.txt".to_string(), "dir/b.txt".to_string(), "new.txt".to_string()];
        let bases = bases(repo.path(), &rels, &HashMap::new());
        assert_eq!(bases[0].head.as_deref(), Some("one\ntwo\nthree\n"));
        assert_eq!(bases[0].index, bases[0].head);
        assert_eq!(bases[0].mode.as_deref(), Some("100644"));
        assert_eq!(bases[1].head.as_deref(), Some("bee\n"));
        assert_eq!((bases[2].head.as_ref(), bases[2].index.as_ref()), (None, None));

        // Stage one of the two hunks by writing the index's text.
        let worktree = "one\n2\nthree\nfour\n";
        let hunks = diff::line_hunks(bases[0].index.as_deref().unwrap(), worktree);
        assert_eq!(hunks.len(), 2);
        let staged = diff::apply(bases[0].index.as_deref().unwrap(), worktree, &[&hunks[1]]);
        write_index(repo.path(), "a.txt", Some(&staged), bases[0].mode.as_deref()).unwrap();
        assert_eq!(repo.git(&["show", ":a.txt"]), "one\ntwo\nthree\nfour\n");
        let file = super::status(repo.path()).unwrap().files.remove(0);
        assert!(file.is_staged() && file.is_unstaged(), "{file:?}");

        // Unstage it again, then stage the whole file and commit it.
        unstage_files(repo.path(), &["a.txt".into()]).unwrap();
        assert_eq!(repo.git(&["show", ":a.txt"]), "one\ntwo\nthree\n");
        stage_files(repo.path(), &["a.txt".into(), "new.txt".into()]).unwrap();
        let id = commit(repo.path(), "Second\n\nBody", false).unwrap();
        assert!(!id.is_empty());
        assert_eq!(last_message(repo.path()).as_deref(), Some("Second\n\nBody"));
        assert!(super::status(repo.path()).unwrap().files.is_empty());
        // Amend keeps one commit.
        repo.write("a.txt", "changed\n");
        stage_files(repo.path(), &["a.txt".into()]).unwrap();
        commit(repo.path(), "Second, amended", true).unwrap();
        assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]).trim(), "2");
        assert!(commit(repo.path(), "  ", false).is_err());

        // Discard Changes takes files back to HEAD.
        repo.write("a.txt", "scribble\n");
        discard_files(repo.path(), &["a.txt".into()]).unwrap();
        assert_eq!(std::fs::read_to_string(repo.root.join("a.txt")).unwrap(), "changed\n");

        // A path git refuses doesn't hide the others' versions.
        let nested = TestRepo::new("nested");
        let inner = repo.root.join("inner");
        std::fs::rename(&nested.root, &inner).unwrap();
        std::fs::write(inner.join("x.txt"), "x\n").unwrap();
        let mixed = super::bases(repo.path(), &["inner/x.txt".into(), "a.txt".into()], &HashMap::new());
        assert_eq!(mixed[1].head.as_deref(), Some("changed\n"));
    }

    #[test]
    fn a_staged_rename_is_compared_with_its_old_path() {
        let repo = TestRepo::new("rename");
        repo.write("old.swift", "let a = 1\nlet b = 2\n");
        repo.write("other.txt", "other\n");
        repo.commit_all("first");
        repo.git(&["mv", "old.swift", "new.swift"]);
        repo.write("new.swift", "let a = 1\nlet b = 2\nlet c = 3\n");
        repo.write("added.txt", "added\n");
        repo.git(&["add", "added.txt"]);
        let rels = vec!["new.swift".to_string(), "added.txt".to_string(), "other.txt".to_string()];
        let bases = bases(repo.path(), &rels, &HashMap::new());
        assert_eq!(bases[0].head.as_deref(), Some("let a = 1\nlet b = 2\n"));
        assert_eq!(bases[0].index, bases[0].head);
        // A new file has no HEAD version; an unchanged one is itself.
        assert_eq!((bases[1].head.as_deref(), bases[1].index.as_deref()), (None, Some("added\n")));
        assert_eq!(bases[2].head.as_deref(), Some("other\n"));
    }

    #[test]
    fn blame_marks_unsaved_lines_as_not_committed() {
        let repo = TestRepo::new("blame");
        repo.write("a.txt", "one\ntwo\n");
        repo.commit_all("first");
        let blame = blame(repo.path(), "a.txt", "one\ninserted\ntwo\n").unwrap();
        let authors: Vec<&str> = blame.lines.iter().map(|&c| blame.commits[c as usize].author.as_str()).collect();
        assert_eq!(authors, vec!["Test Author", "Not Committed Yet", "Test Author"]);
        assert!(blame.commits[blame.lines[1] as usize].uncommitted);
        assert_eq!(blame.commits[blame.lines[0] as usize].summary, "first");
    }

    #[test]
    fn a_switch_local_changes_block_names_them() {
        let repo = TestRepo::new("blocked");
        repo.write("a.txt", "one\n");
        repo.commit_all("first");
        repo.git(&["switch", "-q", "-c", "other"]);
        repo.write("a.txt", "other\n");
        repo.commit_all("other");
        repo.git(&["switch", "-q", "main"]);
        repo.write("a.txt", "local\n");
        let other = Branch { name: "other".into(), remote: false, current: false, upstream: None };
        let err = switch_branch(repo.path(), &other).unwrap_err().to_string();
        assert!(err.ends_with("would be overwritten by checkout: a.txt"), "{err}");
    }

    #[test]
    fn branches_and_a_local_origin() {
        let origin = TestRepo::bare("origin");
        let repo = TestRepo::new("branches");
        repo.write("a.txt", "one\n");
        repo.commit_all("first");
        repo.git(&["remote", "add", "origin", origin.root.to_str().unwrap()]);
        push(repo.path(), "main", "origin/main", false).unwrap();
        assert_eq!(status(repo.path()).unwrap().upstream.as_deref(), Some("origin/main"));

        create_branch(repo.path(), "feature").unwrap();
        assert!(create_branch(repo.path(), "bad name..").is_err());
        let names: Vec<(String, bool, bool)> =
            branches(repo.path()).unwrap().into_iter().map(|b| (b.name, b.remote, b.current)).collect();
        assert_eq!(
            names,
            vec![("feature".into(), false, true), ("main".into(), false, false), ("origin/main".into(), true, false)]
        );
        let main = Branch { name: "main".into(), remote: false, current: false, upstream: None };
        switch_branch(repo.path(), &main).unwrap();
        assert_eq!(status(repo.path()).unwrap().branch.as_deref(), Some("main"));

        // Another clone pushes; fetch sees it, pull brings it in.
        let other = TestRepo::new("other");
        other.git(&["remote", "add", "origin", origin.root.to_str().unwrap()]);
        other.git(&["fetch", "-q", "origin"]);
        other.git(&["checkout", "-q", "-b", "main", "origin/main"]);
        other.write("b.txt", "bee\n");
        other.commit_all("from the other clone");
        other.git(&["push", "-q", "origin", "main"]);
        fetch(repo.path()).unwrap();
        assert_eq!(status(repo.path()).unwrap().behind, 1);
        pull(repo.path(), "origin/main", false).unwrap();
        assert!(repo.root.join("b.txt").exists());
        assert_eq!(status(repo.path()).unwrap().behind, 0);
        assert_eq!(remotes(repo.path()), vec!["origin".to_string()]);

        // A rejected push says why.
        other.write("c.txt", "sea\n");
        other.commit_all("again");
        other.git(&["push", "-q", "origin", "main"]);
        repo.write("d.txt", "dee\n");
        repo.commit_all("diverged");
        let err = push(repo.path(), "main", "origin/main", false).unwrap_err().to_string();
        assert!(err.contains("rejected"), "{err}");
        // Rebase onto it, then the push goes through.
        pull(repo.path(), "origin/main", true).unwrap();
        push(repo.path(), "main", "origin/main", false).unwrap();
        // A remote branch checks out as a local branch tracking it.
        other.git(&["push", "-q", "origin", "main:topic"]);
        fetch(repo.path()).unwrap();
        let topic = branches(repo.path()).unwrap().into_iter().find(|b| b.name == "origin/topic").unwrap();
        switch_branch(repo.path(), &topic).unwrap();
        let status = status(repo.path()).unwrap();
        assert_eq!((status.branch.as_deref(), status.upstream.as_deref()), (Some("topic"), Some("origin/topic")));
    }
}

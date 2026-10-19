//! File changes from FSEvents, for what zcode keeps of files on disk: open
//! buffers, the settings files, the git directory and the build server's
//! project files. Each of those checks its files' times when something under
//! them changes, rather than every second or few, so an idle window doesn't
//! wake for them.
//!
//! One FSEvents stream (through `notify`) watches the folders that cover what
//! is asked for: a project's root whole (`Root`), else a folder asked for, or
//! a file's folder. It's made and changed on a thread of its own, which
//! `start` starts after the first window is up, so startup doesn't wait for
//! it. Until it watches all of a check's files (and for good, for a file in
//! the home folder itself or a folder that doesn't exist, which aren't
//! watched), the check polls at its old interval; FSEvents can also drop
//! events, so a watched check runs every `SAFETY_INTERVAL` anyway.

use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Duration;

use gpui_kit::BackgroundExecutor;
use notify::{RecursiveMode, Watcher as _};

/// How often a check whose files are watched runs anyway.
const SAFETY_INTERVAL: Duration = Duration::from_secs(30);

/// A check's files: `wait` returns when one of them may have changed.
pub struct Changes {
    subscriber: Arc<Subscriber>,
}

struct Subscriber {
    /// Files and folders (a folder's whole tree), canonical.
    interests: Mutex<Vec<PathBuf>>,
    /// Whether the stream watches all of them.
    covered: AtomicBool,
    /// A change is pending (at most one is kept).
    tx: smol::channel::Sender<()>,
    rx: smol::channel::Receiver<()>,
}

/// A project's root, watched whole: files under it are covered by it
/// rather than by their own folders.
pub struct Root {
    path: PathBuf,
}

#[derive(Default)]
struct Hub {
    subscribers: Vec<Weak<Subscriber>>,
    roots: Vec<PathBuf>,
    /// Whether the stream couldn't be made: everything polls.
    failed: bool,
}

static HUB: LazyLock<Mutex<Hub>> = LazyLock::new(Default::default);

/// Requests for the stream's thread, which `start` takes.
static REQUESTS: LazyLock<(Mutex<mpsc::Sender<()>>, Mutex<Option<mpsc::Receiver<()>>>)> = LazyLock::new(|| {
    let (tx, rx) = mpsc::channel();
    (Mutex::new(tx), Mutex::new(Some(rx)))
});

/// Asks the stream's thread to make the stream cover what is asked for now
/// (once it's started).
fn update() {
    REQUESTS.0.lock().unwrap().send(()).ok();
}

/// Starts the stream's thread, which covers what was asked for until now
/// and follows what is asked for later. Called once the first window is up.
pub fn start() {
    let Some(requests) = REQUESTS.1.lock().unwrap().take() else { return };
    update();
    std::thread::Builder::new()
        .name("zcode-fs-watch".into())
        .spawn(move || run(requests))
        .ok();
}

impl Changes {
    /// For `paths` (more can be set later).
    pub fn new(paths: Vec<PathBuf>) -> Self {
        let (tx, rx) = smol::channel::bounded(1);
        let subscriber = Arc::new(Subscriber {
            interests: Mutex::new(vec![]),
            covered: AtomicBool::new(false),
            tx,
            rx,
        });
        HUB.lock().unwrap().subscribers.push(Arc::downgrade(&subscriber));
        let changes = Self { subscriber };
        changes.set_paths(paths);
        changes
    }

    /// The files and folders to follow, when they changed.
    pub fn set_paths(&self, paths: Vec<PathBuf>) {
        let mut paths: Vec<PathBuf> = paths.iter().map(|path| canonical(path)).collect();
        paths.sort();
        paths.dedup();
        let mut interests = self.subscriber.interests.lock().unwrap();
        if *interests == paths {
            return;
        }
        *interests = paths;
        drop(interests);
        // Polled until the stream covers them.
        self.subscriber.covered.store(false, Ordering::Release);
        update();
    }

    /// Until something under the paths changes, or `poll` passes while the
    /// stream doesn't cover them all (`SAFETY_INTERVAL` while it does).
    pub async fn wait(&self, poll: Duration, executor: &BackgroundExecutor) {
        let timeout = if self.subscriber.covered.load(Ordering::Acquire) { SAFETY_INTERVAL } else { poll };
        let timer = executor.timer(timeout);
        futures::future::select(pin!(self.subscriber.rx.recv()), timer).await;
    }

    /// Whether the stream watches all the paths (for the control channel).
    pub fn is_watched(&self) -> bool {
        self.subscriber.covered.load(Ordering::Acquire)
    }
}

impl Drop for Changes {
    fn drop(&mut self) {
        update();
    }
}

impl Root {
    pub fn new(path: &Path) -> Self {
        let path = canonical(path);
        HUB.lock().unwrap().roots.push(path.clone());
        update();
        Self { path }
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let mut hub = HUB.lock().unwrap();
        if let Some(ix) = hub.roots.iter().position(|root| *root == self.path) {
            hub.roots.remove(ix);
        }
        drop(hub);
        update();
    }
}

/// `path` with its links resolved, as FSEvents reports it (`/private/var`
/// for `/var`); a file that doesn't exist through its folder.
fn canonical(path: &Path) -> PathBuf {
    if let Ok(path) = path.canonicalize() {
        return path;
    }
    match (path.parent().and_then(|parent| parent.canonicalize().ok()), path.file_name()) {
        (Some(parent), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    }
}

/// The folders to watch for `interests`, and which interests they cover.
/// A file or folder under a root is covered by the root; the home folder,
/// its ancestors and folders that don't exist aren't watched (the home
/// folder's tree changes all the time).
fn plan(roots: &[PathBuf], interests: &[PathBuf], home: Option<&Path>) -> Vec<PathBuf> {
    let watchable = |dir: &Path| dir.is_dir() && !home.is_some_and(|home| home.starts_with(dir));
    let mut dirs: Vec<PathBuf> = vec![];
    for interest in interests {
        let dir = match roots.iter().filter(|root| interest.starts_with(root)).min_by_key(|root| root.components().count()) {
            Some(root) => root.clone(),
            None if interest.is_dir() => interest.clone(),
            None => match interest.parent() {
                Some(parent) => parent.to_path_buf(),
                None => continue,
            },
        };
        if watchable(&dir) {
            dirs.push(dir);
        }
    }
    dirs.sort();
    dirs.dedup();
    // A folder inside another is covered by it.
    let mut kept: Vec<PathBuf> = vec![];
    for dir in dirs {
        if !kept.iter().any(|outer| dir.starts_with(outer)) {
            kept.push(dir);
        }
    }
    kept
}

/// Whether `dirs` cover `path`.
fn covers(dirs: &[PathBuf], path: &Path) -> bool {
    dirs.iter().any(|dir| path.starts_with(dir) || path.parent().is_some_and(|parent| parent == dir))
}

/// The stream's thread: makes the stream again when what's asked for
/// changes, then tells the checks what it covers.
fn run(requests: mpsc::Receiver<()>) {
    let mut watcher: Option<notify::RecommendedWatcher> = None;
    let mut watched: Vec<PathBuf> = vec![];
    let home = std::env::var_os("HOME").map(PathBuf::from).map(|home| canonical(&home));
    while requests.recv().is_ok() {
        while requests.try_recv().is_ok() {}
        let (roots, subscribers) = {
            let mut hub = HUB.lock().unwrap();
            hub.subscribers.retain(|subscriber| subscriber.strong_count() > 0);
            (hub.roots.clone(), hub.subscribers.iter().filter_map(Weak::upgrade).collect::<Vec<_>>())
        };
        let interests: Vec<PathBuf> =
            subscribers.iter().flat_map(|subscriber| subscriber.interests.lock().unwrap().clone()).collect();
        let dirs = plan(&roots, &interests, home.as_deref());
        if dirs != watched {
            // Never while holding the hub: `notify` stops the old stream's
            // thread, which may be waiting for the hub in `dispatch`.
            watcher = None;
            if !dirs.is_empty() {
                watcher = stream(&dirs);
                if watcher.is_none() {
                    HUB.lock().unwrap().failed = true;
                }
            }
            watched = if watcher.is_some() { dirs } else { vec![] };
        }
        for subscriber in &subscribers {
            let interests = subscriber.interests.lock().unwrap();
            let covered = watcher.is_some() && interests.iter().all(|path| covers(&watched, path));
            drop(interests);
            subscriber.covered.store(covered, Ordering::Release);
            // A new stream starts from now: what changed before it is
            // checked once.
            subscriber.tx.try_send(()).ok();
        }
    }
}

/// A stream for `dirs`, or `None` (logged) if FSEvents refused.
fn stream(dirs: &[PathBuf]) -> Option<notify::RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(|event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            dispatch(&event.paths);
        }
    })
    .inspect_err(|err| log::warn!("file watching: {err}"))
    .ok()?;
    let mut paths = watcher.paths_mut();
    for dir in dirs {
        if let Err(err) = paths.add(dir, RecursiveMode::Recursive) {
            log::warn!("file watching: {}: {err}", dir.display());
        }
    }
    paths.commit().inspect_err(|err| log::warn!("file watching: {err}")).ok()?;
    Some(watcher)
}

/// Wakes the checks with something at or under one of `paths` (or above
/// it: a folder renamed or deleted).
fn dispatch(paths: &[PathBuf]) {
    let hub = HUB.lock().unwrap();
    for subscriber in hub.subscribers.iter().filter_map(Weak::upgrade) {
        let hit = subscriber
            .interests
            .lock()
            .unwrap()
            .iter()
            .any(|interest| paths.iter().any(|path| path.starts_with(interest) || interest.starts_with(path)));
        if hit {
            subscriber.tx.try_send(()).ok();
        }
    }
}

/// For the control channel's `dump`: what the stream covers.
pub fn status() -> serde_json::Value {
    let hub = HUB.lock().unwrap();
    let subscribers: Vec<_> = hub.subscribers.iter().filter_map(Weak::upgrade).collect();
    serde_json::json!({
        "failed": hub.failed,
        "roots": hub.roots.len(),
        "checks": subscribers.len(),
        "watched": subscribers.iter().filter(|s| s.covered.load(Ordering::Acquire)).count(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zcode-fs-watch-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        canonical(&dir)
    }

    #[test]
    fn plans_the_folders_that_cover_what_is_asked() {
        let root = temp_dir("plan");
        let sub = root.join("Sources/App");
        let outside = temp_dir("plan-outside");
        std::fs::create_dir_all(&sub).unwrap();
        let interests = vec![sub.join("A.swift"), root.join("Package.swift"), outside.join("notes.txt"), root.join(".git")];
        // Files under the root are covered by it; another file by its folder.
        assert_eq!(plan(&[root.clone()], &interests, None), vec![root.clone(), outside.clone()]);
        // Without the root, the folders themselves, a folder inside another dropped.
        let dirs = plan(&[], &[sub.join("A.swift"), root.join("Sources"), outside.join("notes.txt")], None);
        assert_eq!(dirs, vec![root.join("Sources"), outside.clone()]);
        // The home folder and its ancestors aren't watched, nor missing folders.
        assert_eq!(plan(&[], &[outside.join("a.txt")], Some(&outside)), Vec::<PathBuf>::new());
        assert_eq!(plan(&[], &[root.join("missing/a.txt")], None), Vec::<PathBuf>::new());
        assert!(covers(&[root.clone()], &sub.join("A.swift")) && !covers(&[sub.clone()], &root.join("Package.swift")));
        std::fs::remove_dir_all(root).ok();
        std::fs::remove_dir_all(outside).ok();
    }

    #[test]
    fn a_change_wakes_the_check_that_follows_the_file() {
        let dir = temp_dir("events");
        let file = dir.join("settings.json");
        std::fs::write(&file, "{}").unwrap();
        start();
        let other = Changes::new(vec![dir.join("other.json")]);
        let changes = Changes::new(vec![file.clone()]);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !changes.is_watched() {
            assert!(std::time::Instant::now() < deadline, "the stream never covered the file");
            std::thread::sleep(Duration::from_millis(20));
        }
        // The new stream's own wake, then quiet.
        std::thread::sleep(Duration::from_millis(300));
        while changes.subscriber.rx.try_recv().is_ok() || other.subscriber.rx.try_recv().is_ok() {}
        std::fs::write(&file, "{\"vim_mode\": true}").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while changes.subscriber.rx.try_recv().is_err() {
            assert!(std::time::Instant::now() < deadline, "no event for the file");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(other.subscriber.rx.try_recv().is_err(), "a check for another file woke");
        std::fs::remove_dir_all(dir).ok();
    }
}

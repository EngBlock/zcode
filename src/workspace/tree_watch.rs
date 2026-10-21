//! The navigator follows files and folders added, removed or renamed
//! outside zcode (a shell, zcode's own terminal, a checkout, a generator):
//! FSEvents reports the paths (`fs_watch::TreeChanges`), and the tree is
//! scanned again only when one of them is a file the navigator would list
//! and doesn't, or lists and is gone. A burst (a checkout, a build) is
//! looked at once it settles, and at most once a second.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::*;

use super::Workspace;

/// How long a burst of changes has to settle.
const SETTLE: Duration = Duration::from_millis(250);
/// The least time between two scans for changes.
const SPACING: Duration = Duration::from_secs(1);

impl Workspace {
    pub(super) fn watch_tree(&mut self, cx: &mut Context<Self>) {
        let changes = crate::fs_watch::TreeChanges::new(&self.project.root);
        let task = cx.spawn(async move |this, cx| {
            loop {
                changes.wait().await;
                cx.background_executor().timer(SETTLE).await;
                let (paths, overflow) = changes.take();
                if paths.is_empty() && !overflow {
                    continue;
                }
                let Ok((root, files)) = this.update(cx, |this, _| (this.project.root.clone(), this.files.clone())) else { break };
                let affected = overflow || cx.background_spawn(async move { tree_affected(&root, &files, &paths) }).await;
                if affected {
                    if this.update(cx, |this, cx| this.scan(true, cx)).is_err() {
                        break;
                    }
                    cx.background_executor().timer(SPACING).await;
                }
            }
        });
        self._tasks.push(task);
    }
}

/// Whether any of `paths` (canonical, as FSEvents reports them) changes
/// what the navigator lists: a file it would list and doesn't, a file or
/// folder it lists that's gone, a new folder. `files` are relative to
/// `root`, as `project::scan_files` returns them.
fn tree_affected(root: &Path, files: &[PathBuf], paths: &[PathBuf]) -> bool {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let listed: HashSet<&Path> = files.iter().map(PathBuf::as_path).collect();
    let ignore = {
        let mut builder = ignore::gitignore::GitignoreBuilder::new(&root);
        builder.add(root.join(".gitignore"));
        builder.add(root.join(".git/info/exclude"));
        builder.build().ok()
    };
    paths.iter().any(|path| {
        if *path == root || root.starts_with(path) {
            // The project folder itself moved or went.
            return true;
        }
        let Ok(rel) = path.strip_prefix(&root) else { return false };
        let skipped = rel.components().any(|c| {
            let name = c.as_os_str().to_string_lossy();
            name.starts_with('.') || crate::project::skips_dir(&name)
        });
        if skipped {
            return false;
        }
        match std::fs::symlink_metadata(path) {
            Ok(meta) => {
                let ignored = ignore.as_ref().is_some_and(|i| i.matched_path_or_any_parents(rel, meta.is_dir()).is_ignore());
                if ignored {
                    false
                } else if meta.is_dir() {
                    // A folder moved in, or made: what's in it isn't listed yet.
                    !files.iter().any(|file| file.starts_with(rel))
                } else {
                    !listed.contains(rel)
                }
            }
            // Gone: a file or a folder the navigator lists.
            Err(_) => listed.contains(rel) || files.iter().any(|file| file.starts_with(rel)),
        }
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::tree_affected;

    #[test]
    fn only_changes_to_what_the_navigator_lists_count() {
        let root = std::env::temp_dir().join(format!("zcode-tree-watch-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("Sources/App")).unwrap();
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::create_dir_all(root.join(".build")).unwrap();
        std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
        for file in ["Sources/App/A.swift", "Sources/App/New.swift", "build/out.o", ".build/x.o", "Sources/App/.DS_Store"] {
            std::fs::write(root.join(file), "").unwrap();
        }
        let root = root.canonicalize().unwrap();
        let files: Vec<PathBuf> = vec!["Sources/App/A.swift".into(), "Gone.swift".into(), "Old/B.swift".into()];
        let affected = |rel: &str| tree_affected(&root, &files, &[root.join(rel)]);
        // Listed already, ignored, hidden or skipped: nothing to do.
        assert!(!affected("Sources/App/A.swift"));
        assert!(!affected("build/out.o"));
        assert!(!affected(".build/x.o"));
        assert!(!affected("Sources/App/.DS_Store"));
        // A temporary file that came and went.
        assert!(!affected("Sources/App/.A.swift.tmp"));
        assert!(!affected("Sources/App/Scratch.swift"));
        // A new file, one deleted, a folder deleted or renamed away.
        assert!(affected("Sources/App/New.swift"));
        assert!(affected("Gone.swift"));
        assert!(affected("Old"));
        // A folder that appeared (moved in, or made).
        std::fs::create_dir_all(root.join("Moved/Deep")).unwrap();
        assert!(affected("Moved"));
        // Known folders getting a change of their own don't count.
        assert!(!affected("Sources/App"));
        // Outside the project.
        assert!(!tree_affected(&root, &files, &[std::env::temp_dir().join("elsewhere.swift")]));
        std::fs::remove_dir_all(&root).ok();
    }
}

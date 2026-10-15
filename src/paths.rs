//! Locations zcode reads and writes outside the user's project.

use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

pub fn support_dir() -> PathBuf {
    home().join("Library/Application Support/zcode")
}

pub fn logs_dir() -> PathBuf {
    home().join("Library/Logs/zcode")
}

pub fn cache_dir() -> PathBuf {
    home().join("Library/Caches/zcode")
}

/// Per-project cache directory (compilation database, build logs, ...).
pub fn project_cache_dir(root: &Path) -> PathBuf {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut hasher);
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "root".into());
    cache_dir()
        .join("projects")
        .join(format!("{name}-{:016x}", hasher.finish()))
}

/// `to` expressed relative to `from` (both absolute), using `..` segments.
pub fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for component in &to[common..] {
        out.push(component.as_os_str());
    }
    out
}

/// Shortens a path under `$HOME` to `~/...` for display.
pub fn display_path(path: &Path) -> String {
    let home = home();
    match path.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths() {
        assert_eq!(
            relative_path(Path::new("/a/b/c"), Path::new("/a/x/y")),
            PathBuf::from("../../x/y")
        );
        assert_eq!(
            relative_path(Path::new("/a"), Path::new("/a/b")),
            PathBuf::from("b")
        );
    }
}

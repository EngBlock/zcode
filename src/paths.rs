//! Locations zcode reads and writes outside the user's project.

use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// `ZCODE_CONFIG_DIR`: a folder that stands in for both the configuration
/// folder and the session's, so scripted runs (e2e) never touch the user's.
fn config_override() -> Option<PathBuf> {
    std::env::var_os("ZCODE_CONFIG_DIR").filter(|dir| !dir.is_empty()).map(PathBuf::from)
}

/// Whether `ZCODE_CONFIG_DIR` stands in for the user's folders.
pub fn has_config_override() -> bool {
    config_override().is_some()
}

/// Settings, key bindings and themes, as Zed keeps them: `~/.config/zcode`.
pub fn config_dir() -> PathBuf {
    config_override().unwrap_or_else(|| home().join(".config/zcode"))
}

/// The session (recent projects, each project's tabs and breakpoints).
pub fn support_dir() -> PathBuf {
    match config_override() {
        Some(dir) => dir.join("support"),
        None => home().join("Library/Application Support/zcode"),
    }
}

pub fn logs_dir() -> PathBuf {
    home().join("Library/Logs/zcode")
}

pub fn cache_dir() -> PathBuf {
    home().join("Library/Caches/zcode")
}

/// Recordings Product ▸ Profile makes (Instruments opens them from here),
/// in the run's own folder when `ZCODE_CONFIG_DIR` stands in.
pub fn traces_dir() -> PathBuf {
    match config_override() {
        Some(dir) => dir.join("traces"),
        None => cache_dir().join("Traces"),
    }
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

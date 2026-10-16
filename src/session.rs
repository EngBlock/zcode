//! Persisted per-project UI state and the recent-projects list.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectSession {
    pub open_files: Vec<PathBuf>,
    pub active_file: Option<PathBuf>,
    pub scheme: Option<String>,
    pub destination: Option<String>,
    pub configuration: Option<String>,
    /// The chosen test plan, when not the scheme's default.
    pub test_plan: Option<String>,
    /// Edit Scheme options of schemes without a scheme file (Swift
    /// packages, schemes xcodebuild makes up), by scheme.
    pub scheme_options: std::collections::BTreeMap<String, crate::scheme::SchemeOptions>,
    pub show_navigator: Option<bool>,
    pub show_debug_area: Option<bool>,
    pub show_inspector: Option<bool>,
    /// File → 1-based breakpoint lines, as sessions saved them before
    /// breakpoints had options; read into `breakpoints_v2`.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub breakpoints: std::collections::BTreeMap<PathBuf, Vec<u32>>,
    /// Every breakpoint with its options (Xcode's file is Breakpoints_v2).
    pub breakpoints_v2: Vec<crate::debugger::Breakpoint>,
    pub breakpoints_active: Option<bool>,
    /// Bookmarks and bookmarked searches, in the navigator's order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bookmarks: Vec<crate::bookmarks::Entry>,
}

impl ProjectSession {
    /// The saved breakpoints, including ones from older sessions.
    pub fn all_breakpoints(&self) -> Vec<crate::debugger::Breakpoint> {
        use crate::debugger::{Breakpoint, breakpoints::Kind};
        let mut all = self.breakpoints_v2.clone();
        for (path, lines) in &self.breakpoints {
            for &line in lines {
                if !all.iter().any(|bp| bp.is_at(path, line)) {
                    all.push(Breakpoint::new(Kind::Line { path: path.clone(), line }));
                }
            }
        }
        all
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    /// Most recent first. Paths are what the user opened (folder, project...).
    pub recent: Vec<PathBuf>,
    pub projects: HashMap<String, ProjectSession>,
    pub font_size: Option<f32>,
}

fn session_path() -> PathBuf {
    crate::paths::support_dir().join("session.json")
}

impl Session {
    pub fn load() -> Self {
        std::fs::read(session_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = session_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            std::fs::write(path, bytes).ok();
        }
    }

    pub fn note_recent(&mut self, opened: &Path) {
        self.recent.retain(|p| p != opened);
        self.recent.insert(0, opened.to_path_buf());
        self.recent.truncate(12);
    }

    pub fn project(&self, root: &Path) -> ProjectSession {
        self.projects
            .get(&root.to_string_lossy().to_string())
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_project(&mut self, root: &Path, state: ProjectSession) {
        self.projects.insert(root.to_string_lossy().to_string(), state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakpoints_from_older_sessions_are_kept() {
        let old: ProjectSession =
            serde_json::from_str(r#"{ "breakpoints": { "/a.swift": [3, 9] }, "breakpoints_active": true }"#).unwrap();
        let all = old.all_breakpoints();
        assert_eq!(all.len(), 2);
        assert!(all[0].is_at(Path::new("/a.swift"), 3) && all[1].is_at(Path::new("/a.swift"), 9));
        let saved = ProjectSession { breakpoints_v2: all, ..Default::default() };
        let text = serde_json::to_string(&saved).unwrap();
        assert!(!text.contains("\"breakpoints\":"), "the old map isn't written: {text}");
        let reread: ProjectSession = serde_json::from_str(&text).unwrap();
        assert_eq!(reread.all_breakpoints(), saved.breakpoints_v2);
    }
}

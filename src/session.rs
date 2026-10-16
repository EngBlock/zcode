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
    pub show_navigator: Option<bool>,
    pub show_debug_area: Option<bool>,
    pub show_inspector: Option<bool>,
    /// File → 1-based breakpoint lines.
    pub breakpoints: std::collections::BTreeMap<PathBuf, Vec<u32>>,
    pub breakpoints_active: Option<bool>,
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

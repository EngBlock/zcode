//! Persisted per-project UI state and the recent-projects list.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectSession {
    /// Every open file and the focused one, as sessions saved them before
    /// editor panes (and still do, for older versions); `panes` wins.
    pub open_files: Vec<PathBuf>,
    pub active_file: Option<PathBuf>,
    /// The editor area's panes and splits with their tabs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub panes: Option<PaneLayout>,
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
    /// The debug area's terminals, as the folder each was in, and whether
    /// its Terminal tab was the one shown.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub terminals: Vec<PathBuf>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub terminal_tab: bool,
}

/// The editor area as saved: a pane, or panes side by side or stacked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaneLayout {
    Pane(PaneState),
    Split {
        axis: crate::workspace::SplitAxis,
        members: Vec<PaneLayout>,
        /// The members' sizes along the axis, in points, as last dragged.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sizes: Vec<f32>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaneState {
    pub files: Vec<PathBuf>,
    pub active: Option<PathBuf>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub focused: bool,
    /// An assistant: the pane it assists (its index in layout order), and
    /// whether it follows that pane's counterparts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistant_of: Option<usize>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub counterparts: bool,
    /// Terminal tabs among the files.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub terminals: Vec<SavedTerminal>,
}

/// A terminal tab: where it was among the pane's tabs and the folder its
/// shell was in (a fresh shell starts there; processes aren't restored).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedTerminal {
    pub position: usize,
    pub dir: PathBuf,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub active: bool,
}

impl PaneLayout {
    /// The panes in layout order (left to right, top to bottom).
    pub fn panes(&self) -> Vec<&PaneState> {
        match self {
            PaneLayout::Pane(pane) => vec![pane],
            PaneLayout::Split { members, .. } => members.iter().flat_map(PaneLayout::panes).collect(),
        }
    }
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
    // The editor toggles that were here (font size, blame, Authors, Code
    // Review's layout, inlay hints, Vim mode) are settings now:
    // `settings::migrate_session` moves them into settings.json once.
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

    #[test]
    fn pane_layouts_round_trip_and_older_sessions_have_none() {
        use crate::workspace::SplitAxis;
        let old: ProjectSession = serde_json::from_str(r#"{ "open_files": ["/a.swift"], "active_file": "/a.swift" }"#).unwrap();
        assert_eq!(old.panes, None);
        let pane = |files: &[&str], focused| {
            PaneLayout::Pane(PaneState {
                files: files.iter().map(PathBuf::from).collect(),
                active: files.first().map(PathBuf::from),
                focused,
                ..Default::default()
            })
        };
        let layout = PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            members: vec![
                pane(&["/a.swift", "/b.swift"], false),
                PaneLayout::Split { axis: SplitAxis::Vertical, members: vec![pane(&["/a.swift"], true), pane(&[], false)], sizes: vec![] },
            ],
            sizes: vec![640., 480.],
        };
        let saved = ProjectSession { panes: Some(layout.clone()), ..Default::default() };
        let text = serde_json::to_string(&saved).unwrap();
        assert!(text.contains(r#""axis":"vertical""#) && !text.contains("counterparts"), "{text}");
        assert_eq!(text.matches("sizes").count(), 1, "{text}");
        assert!(!text.contains("terminal"), "{text}");
        let reread: ProjectSession = serde_json::from_str(&text).unwrap();
        assert_eq!(reread.panes, Some(layout));
        let panes = reread.panes.as_ref().unwrap().panes();
        assert_eq!(panes.len(), 3);
        assert!(panes[1].focused && panes[2].files.is_empty());
    }

    #[test]
    fn terminals_are_kept_as_their_folders() {
        let pane = PaneState {
            files: vec![PathBuf::from("/a.swift")],
            terminals: vec![SavedTerminal { position: 1, dir: PathBuf::from("/project/Sources"), active: true }],
            ..Default::default()
        };
        let saved = ProjectSession {
            panes: Some(PaneLayout::Pane(pane.clone())),
            terminals: vec![PathBuf::from("/project")],
            terminal_tab: true,
            ..Default::default()
        };
        let text = serde_json::to_string(&saved).unwrap();
        let reread: ProjectSession = serde_json::from_str(&text).unwrap();
        assert_eq!(reread.panes, Some(PaneLayout::Pane(pane)));
        assert_eq!(reread.terminals, vec![PathBuf::from("/project")]);
        assert!(reread.terminal_tab);
    }
}

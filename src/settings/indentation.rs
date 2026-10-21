//! How a file is indented: tabs or spaces, and how many. From the first of
//! these that says:
//!
//! 1. the project's `.zcode/settings.json` (`tab_size`, `hard_tabs`);
//! 2. `.editorconfig` files (`indent_style`, `indent_size`, `tab_width`);
//! 3. your own settings file;
//! 4. the file's text, read when it opens (`zcode_editor::detect_indent`);
//! 5. the defaults (4 spaces).
//!
//! Each of tabs-or-spaces and the width is decided on its own, so a project
//! that only sets `hard_tabs` still gets its width from below. A project's
//! conventions win over your preference, and your preference over a guess.

use std::path::Path;

use zcode_editor::DetectedIndent;

use super::Settings;

/// `tab_size` and `hard_tabs` as one settings file says them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pinned {
    pub tab_size: Option<u32>,
    pub hard_tabs: Option<bool>,
}

/// What a window's settings files say about indentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IndentSettings {
    pub project: Pinned,
    pub user: Pinned,
    /// With the defaults under them.
    pub tab_size: u32,
    pub hard_tabs: bool,
}

impl IndentSettings {
    pub fn new(settings: &Settings, user: Pinned, project: Pinned) -> Self {
        Self { project, user, tab_size: settings.tab_size, hard_tabs: settings.hard_tabs }
    }
}

/// What a file's `.editorconfig` files say about it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditorConfigIndent {
    pub hard_tabs: Option<bool>,
    /// `indent_size`; `None` for `tab`, which means `tab_width`.
    pub indent_size: Option<u32>,
    pub tab_width: Option<u32>,
}

impl EditorConfigIndent {
    fn width(&self) -> Option<u32> {
        match self.hard_tabs {
            Some(true) => self.tab_width.or(self.indent_size),
            _ => self.indent_size.or(self.tab_width),
        }
    }
}

/// A file's indentation: its width and whether Tab types a tab.
pub fn resolve(settings: &IndentSettings, editorconfig: EditorConfigIndent, detected: Option<DetectedIndent>) -> (u32, bool) {
    let hard_tabs = settings
        .project
        .hard_tabs
        .or(editorconfig.hard_tabs)
        .or(settings.user.hard_tabs)
        .or(detected.map(|d| d == DetectedIndent::Tabs))
        .unwrap_or(settings.hard_tabs);
    let detected_width = match detected {
        Some(DetectedIndent::Spaces(width)) if !hard_tabs => Some(width as u32),
        _ => None,
    };
    let tab_size = settings
        .project
        .tab_size
        .or(editorconfig.width())
        .or(settings.user.tab_size)
        .or(detected_width)
        .unwrap_or(settings.tab_size);
    (tab_size, hard_tabs)
}

/// The indentation the `.editorconfig` files from `path`'s folder up say
/// for it (EditorConfig's rules: nearer files win, later sections win, a
/// file with `root = true` ends the search). Globs are `*`, `**`, `?`,
/// `[…]` and `{a,b}`; numeric ranges (`{1..3}`) match nothing.
pub fn editorconfig(path: &Path) -> EditorConfigIndent {
    let mut files = vec![];
    let mut dir = path.parent();
    while let Some(folder) = dir {
        if let Ok(text) = std::fs::read_to_string(folder.join(".editorconfig")) {
            let root = is_root(&text);
            files.push((folder.to_path_buf(), text));
            if root {
                break;
            }
        }
        dir = folder.parent();
    }
    let mut indent = EditorConfigIndent::default();
    for (folder, text) in files.iter().rev() {
        let Ok(relative) = path.strip_prefix(folder) else { continue };
        apply(&mut indent, text, relative);
    }
    indent
}

fn pairs(text: &str) -> impl Iterator<Item = Result<(String, String), String>> + '_ {
    text.lines().filter_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            return None;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            return Some(Err(section.to_string()));
        }
        let (key, value) = line.split_once(['=', ':'])?;
        Some(Ok((key.trim().to_lowercase(), value.trim().to_lowercase())))
    })
}

fn is_root(text: &str) -> bool {
    pairs(text).map_while(Result::ok).any(|(key, value)| key == "root" && value == "true")
}

/// What one `.editorconfig` says for the file at `relative` to its folder.
fn apply(indent: &mut EditorConfigIndent, text: &str, relative: &Path) {
    let mut matches = false;
    for pair in pairs(text) {
        match pair {
            Err(section) => matches = section_matches(&section, relative),
            Ok((key, value)) if matches => {
                let number = || value.parse::<u32>().ok().filter(|n| (1..=16).contains(n));
                match key.as_str() {
                    "indent_style" => {
                        indent.hard_tabs = match value.as_str() {
                            "tab" => Some(true),
                            "space" => Some(false),
                            _ => None,
                        }
                    }
                    "indent_size" => indent.indent_size = number(),
                    "tab_width" => indent.tab_width = number(),
                    _ => {}
                }
            }
            Ok(_) => {}
        }
    }
}

fn section_matches(section: &str, relative: &Path) -> bool {
    // A glob without a slash matches the name in any folder; one with a
    // slash is from the `.editorconfig`'s folder.
    let glob = if section.contains('/') {
        section.trim_start_matches('/').to_string()
    } else {
        format!("**/{section}")
    };
    globset::GlobBuilder::new(&glob)
        .literal_separator(true)
        .build()
        .is_ok_and(|glob| glob.compile_matcher().is_match(relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(project: Pinned, user: Pinned) -> IndentSettings {
        IndentSettings { project, user, tab_size: project.tab_size.or(user.tab_size).unwrap_or(4), hard_tabs: project.hard_tabs.or(user.hard_tabs).unwrap_or(false) }
    }

    #[test]
    fn the_first_that_says_wins() {
        let none = Pinned::default();
        let two = Some(DetectedIndent::Spaces(2));
        // Nothing set: the file's own.
        assert_eq!(resolve(&settings(none, none), Default::default(), two), (2, false));
        assert_eq!(resolve(&settings(none, none), Default::default(), Some(DetectedIndent::Tabs)), (4, true));
        // Nothing to go by: the defaults.
        assert_eq!(resolve(&settings(none, none), Default::default(), None), (4, false));
        // Your settings over the guess.
        let user = Pinned { tab_size: Some(8), hard_tabs: None };
        assert_eq!(resolve(&settings(none, user), Default::default(), two), (8, false));
        // .editorconfig over yours, the project's over .editorconfig.
        let config = EditorConfigIndent { hard_tabs: Some(true), indent_size: None, tab_width: Some(3) };
        assert_eq!(resolve(&settings(none, user), config, two), (3, true));
        let project = Pinned { tab_size: None, hard_tabs: Some(false) };
        assert_eq!(resolve(&settings(project, user), config, two), (3, false));
    }

    #[test]
    fn editorconfig_files_from_the_folder_up() {
        let dir = std::env::temp_dir().join(format!("zcode-editorconfig-{}", std::process::id()));
        let web = dir.join("web").join("src");
        std::fs::create_dir_all(&web).unwrap();
        std::fs::write(
            dir.join(".editorconfig"),
            "root = true\n\n[*]\nindent_style = space\nindent_size = 4\n\n[*.{ts,tsx}]\nindent_size = 2\n\n[Makefile]\nindent_style = tab\n",
        )
        .unwrap();
        std::fs::write(dir.join("web").join(".editorconfig"), "[src/*.ts]\nindent_style = tab\ntab_width = 8\n").unwrap();
        assert_eq!(editorconfig(&dir.join("a.swift")), EditorConfigIndent { hard_tabs: Some(false), indent_size: Some(4), tab_width: None });
        assert_eq!(editorconfig(&dir.join("b.ts")).indent_size, Some(2));
        assert_eq!(editorconfig(&dir.join("x").join("Makefile")).hard_tabs, Some(true));
        // The nearer file wins.
        assert_eq!(editorconfig(&web.join("main.ts")), EditorConfigIndent { hard_tabs: Some(true), indent_size: Some(2), tab_width: Some(8) });
        std::fs::remove_dir_all(&dir).ok();
    }
}

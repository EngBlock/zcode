//! Bookmarks as Xcode 27 keeps them: lines and files, each with a
//! description and a completion mark, and find queries saved with their
//! results (a search group that Find Again refreshes). They are saved per
//! project in the session.
//!
//! The Bookmarks navigator lists bookmarks grouped by file, in the order
//! they were added or dragged into, then each search group with its results.
//! Line bookmarks follow edits through the same anchors breakpoints use.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::search::SearchOptions;

/// What a bookmark points at.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Location {
    /// A file and 1-based line.
    Line { path: PathBuf, line: u32 },
    File { path: PathBuf },
}

impl Location {
    pub fn path(&self) -> &Path {
        match self {
            Self::Line { path, .. } | Self::File { path } => path,
        }
    }

    pub fn line(&self) -> Option<u32> {
        match self {
            Self::Line { line, .. } => Some(*line),
            Self::File { .. } => None,
        }
    }

    fn path_mut(&mut self) -> &mut PathBuf {
        match self {
            Self::Line { path, .. } | Self::File { path } => path,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    /// Stable for the life of the window; not saved.
    #[serde(skip)]
    pub id: u64,
    pub location: Location,
    /// Xcode's "description": the line's text (or the selection, or the
    /// file's name) until edited.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub completed: bool,
}

impl Bookmark {
    pub fn new(location: Location, title: impl Into<String>) -> Self {
        Self { id: 0, location, title: title.into(), completed: false }
    }

    /// The description, or where the bookmark is when it has none.
    pub fn display_title(&self) -> String {
        if !self.title.trim().is_empty() {
            return self.title.clone();
        }
        let name = file_name(self.location.path());
        match self.location.line() {
            Some(line) => format!("{name}:{line}"),
            None => name,
        }
    }
}

/// A find query bookmarked from the Find navigator, with its results.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchGroup {
    #[serde(skip)]
    pub id: u64,
    pub query: String,
    #[serde(default)]
    pub options: SearchOptions,
    /// The group's description; the query until edited.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default)]
    pub results: Vec<Bookmark>,
}

impl SearchGroup {
    pub fn display_title(&self) -> String {
        if self.title.trim().is_empty() { format!("“{}”", self.query) } else { self.title.clone() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
pub enum Entry {
    Bookmark(Bookmark),
    Search(SearchGroup),
}

/// A group of rows in the navigator: a file's bookmarks, or a search.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SectionKey {
    File(PathBuf),
    Search(u64),
}

/// How Sort By orders bookmarks (Xcode's Sort By Name / Type / Completion Status).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortBy {
    Name,
    Type,
    CompletionStatus,
}

/// A navigator section and its bookmarks, in display order.
#[derive(Debug)]
pub struct Section<'a> {
    pub key: SectionKey,
    pub search: Option<&'a SearchGroup>,
    pub bookmarks: Vec<&'a Bookmark>,
}

impl Section<'_> {
    pub fn title(&self) -> String {
        match (&self.key, self.search) {
            (_, Some(search)) => search.display_title(),
            (SectionKey::File(path), _) => file_name(path),
            (SectionKey::Search(_), None) => String::new(),
        }
    }
}

pub fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().to_string()
}

#[derive(Default)]
pub struct Bookmarks {
    pub entries: Vec<Entry>,
    next_id: u64,
}

impl Bookmarks {
    /// Replaces every bookmark (restoring a session).
    pub fn restore(&mut self, entries: Vec<Entry>) {
        self.entries.clear();
        for mut entry in entries {
            match &mut entry {
                Entry::Bookmark(b) => b.id = self.take_id(),
                Entry::Search(group) => {
                    group.id = self.take_id();
                    for b in &mut group.results {
                        b.id = self.take_id();
                    }
                }
            }
            self.entries.push(entry);
        }
    }

    fn take_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every bookmark, search results included.
    pub fn all(&self) -> impl Iterator<Item = &Bookmark> {
        self.entries.iter().flat_map(|entry| match entry {
            Entry::Bookmark(b) => std::slice::from_ref(b).iter(),
            Entry::Search(group) => group.results.iter(),
        })
    }

    fn all_mut(&mut self) -> impl Iterator<Item = &mut Bookmark> {
        self.entries.iter_mut().flat_map(|entry| match entry {
            Entry::Bookmark(b) => std::slice::from_mut(b).iter_mut(),
            Entry::Search(group) => group.results.iter_mut(),
        })
    }

    pub fn get(&self, id: u64) -> Option<&Bookmark> {
        self.all().find(|b| b.id == id)
    }

    pub fn search(&self, id: u64) -> Option<&SearchGroup> {
        self.entries.iter().find_map(|entry| match entry {
            Entry::Search(group) if group.id == id => Some(group),
            _ => None,
        })
    }

    /// The bookmark Bookmark Line or Bookmark File would add again (search
    /// results don't count: a result can also be bookmarked on its own).
    pub fn find(&self, location: &Location) -> Option<&Bookmark> {
        self.entries.iter().find_map(|entry| match entry {
            Entry::Bookmark(b) if b.location == *location => Some(b),
            _ => None,
        })
    }

    /// The line bookmarks in `path` with their 1-based lines, results included.
    pub fn lines_in<'a>(&'a self, path: &'a Path) -> impl Iterator<Item = (u64, u32)> + 'a {
        self.all().filter(move |b| b.location.path() == path).filter_map(|b| Some((b.id, b.location.line()?)))
    }

    /// Adds a bookmark, unless one is already there; returns its id and
    /// whether it was added.
    pub fn add(&mut self, location: Location, title: impl Into<String>) -> (u64, bool) {
        if let Some(existing) = self.find(&location) {
            return (existing.id, false);
        }
        let mut bookmark = Bookmark::new(location, title);
        bookmark.id = self.take_id();
        let id = bookmark.id;
        self.entries.push(Entry::Bookmark(bookmark));
        (id, true)
    }

    /// Bookmarks a find query with its results (file, 1-based line and the
    /// line's text); returns the group's id.
    pub fn add_search(&mut self, query: String, options: SearchOptions, results: Vec<(PathBuf, u32, String)>) -> u64 {
        let id = self.take_id();
        let mut group = SearchGroup { id, query, options, title: String::new(), results: vec![] };
        group.results = self.result_bookmarks(results, &[]);
        self.entries.push(Entry::Search(group));
        id
    }

    /// Find Again: new results for a search group. Results still on the
    /// same line keep their description and completion mark.
    pub fn replace_search_results(&mut self, id: u64, results: Vec<(PathBuf, u32, String)>) {
        let Some(old) = self.search(id).map(|g| g.results.clone()) else { return };
        let results = self.result_bookmarks(results, &old);
        if let Some(Entry::Search(group)) = self.entries.iter_mut().find(|e| matches!(e, Entry::Search(g) if g.id == id)) {
            group.results = results;
        }
    }

    fn result_bookmarks(&mut self, results: Vec<(PathBuf, u32, String)>, previous: &[Bookmark]) -> Vec<Bookmark> {
        let mut out: Vec<Bookmark> = vec![];
        for (path, line, text) in results {
            let location = Location::Line { path, line };
            // One bookmark per line, however many matches it has.
            if out.iter().any(|b| b.location == location) {
                continue;
            }
            let mut bookmark = previous
                .iter()
                .find(|b| b.location == location)
                .cloned()
                .unwrap_or_else(|| Bookmark::new(location, text.trim()));
            bookmark.id = self.take_id();
            out.push(bookmark);
        }
        out
    }

    /// Removes a bookmark or a whole search group.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.count();
        self.entries.retain(|entry| match entry {
            Entry::Bookmark(b) => b.id != id,
            Entry::Search(group) => group.id != id,
        });
        for entry in &mut self.entries {
            if let Entry::Search(group) = entry {
                group.results.retain(|b| b.id != id);
            }
        }
        self.count() != before
    }

    fn count(&self) -> usize {
        self.entries.len() + self.all().count()
    }

    /// Removes the bookmarks of a navigator section.
    pub fn remove_section(&mut self, key: &SectionKey) {
        self.entries.retain(|entry| entry_section(entry) != *key);
    }

    /// Delete All Completed Items.
    pub fn remove_completed(&mut self) -> usize {
        let before = self.count();
        self.entries.retain(|entry| !matches!(entry, Entry::Bookmark(b) if b.completed));
        for entry in &mut self.entries {
            if let Entry::Search(group) = entry {
                group.results.retain(|b| !b.completed);
            }
        }
        before - self.count()
    }

    /// Edit Description, for a bookmark or a search group.
    pub fn set_title(&mut self, id: u64, title: &str) {
        let title = title.trim().to_string();
        for entry in &mut self.entries {
            if let Entry::Search(group) = entry
                && group.id == id
            {
                group.title = if title == group.query { String::new() } else { title };
                return;
            }
        }
        if let Some(b) = self.all_mut().find(|b| b.id == id) {
            b.title = title;
        }
    }

    /// Mark as Complete / Incomplete.
    pub fn set_completed(&mut self, id: u64, completed: bool) {
        if let Some(b) = self.all_mut().find(|b| b.id == id) {
            b.completed = completed;
        }
    }

    /// Moves line bookmarks after an edit in `path` (`None`: the line was
    /// deleted, and the bookmark with it). Lines joined by the edit keep one
    /// bookmark each, the first.
    pub fn move_lines(&mut self, path: &Path, moves: &[(u64, Option<u32>)]) {
        let deleted: Vec<u64> = moves.iter().filter(|(_, line)| line.is_none()).map(|(id, _)| *id).collect();
        for b in self.all_mut() {
            if let Some((_, Some(line))) = moves.iter().find(|(id, _)| *id == b.id)
                && let Location::Line { line: current, .. } = &mut b.location
            {
                *current = *line;
            }
        }
        for id in deleted {
            self.remove(id);
        }
        let mut seen = std::collections::HashSet::new();
        self.entries.retain(|entry| match entry {
            Entry::Bookmark(b) if b.location.path() == path => seen.insert(b.location.clone()),
            _ => true,
        });
        for entry in &mut self.entries {
            if let Entry::Search(group) = entry {
                let mut seen = std::collections::HashSet::new();
                group.results.retain(|b| b.location.path() != path || seen.insert(b.location.clone()));
            }
        }
    }

    /// Keeps bookmarks with a file or folder that was renamed or moved.
    pub fn move_files(&mut self, from: &Path, to: &Path) -> bool {
        let mut moved = false;
        for b in self.all_mut() {
            let path = b.location.path_mut();
            if let Ok(rest) = path.strip_prefix(from) {
                *path = if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) };
                moved = true;
            }
        }
        moved
    }

    /// Deletes the bookmarks in a file or folder that was deleted.
    pub fn remove_in(&mut self, folder: &Path) -> bool {
        let before = self.count();
        self.entries.retain(|entry| !matches!(entry, Entry::Bookmark(b) if b.location.path().starts_with(folder)));
        for entry in &mut self.entries {
            if let Entry::Search(group) = entry {
                group.results.retain(|b| !b.location.path().starts_with(folder));
            }
        }
        self.count() != before
    }

    /// The navigator's sections: files in the order their first bookmark
    /// appears, and search groups where they were added.
    pub fn sections(&self) -> Vec<Section<'_>> {
        let mut sections: Vec<Section> = vec![];
        for entry in &self.entries {
            let key = entry_section(entry);
            match entry {
                Entry::Bookmark(b) => match sections.iter_mut().find(|s| s.key == key) {
                    Some(section) => section.bookmarks.push(b),
                    None => sections.push(Section { key, search: None, bookmarks: vec![b] }),
                },
                Entry::Search(group) => sections.push(Section { key, search: Some(group), bookmarks: group.results.iter().collect() }),
            }
        }
        sections
    }

    /// Every bookmark in the navigator's order.
    pub fn in_order(&self) -> Vec<&Bookmark> {
        self.sections().into_iter().flat_map(|s| s.bookmarks).collect()
    }

    /// Drag and drop within a section: moves bookmark `id` to where
    /// `before` is (after it when dragged downwards), or to the section's
    /// end when `before` is `None`. Returns whether anything moved.
    pub fn move_bookmark(&mut self, id: u64, before: Option<u64>) -> bool {
        if Some(id) == before {
            return false;
        }
        // Inside a search group.
        for entry in &mut self.entries {
            let Entry::Search(group) = entry else { continue };
            let Some(from) = group.results.iter().position(|b| b.id == id) else { continue };
            let to = match before {
                Some(before) => match group.results.iter().position(|b| b.id == before) {
                    Some(to) => to,
                    None => return false,
                },
                None => group.results.len() - 1,
            };
            let bookmark = group.results.remove(from);
            group.results.insert(to.min(group.results.len()), bookmark);
            return from != to;
        }
        // Top-level bookmarks of the same file.
        let Some(from) = self.entries.iter().position(|e| matches!(e, Entry::Bookmark(b) if b.id == id)) else { return false };
        let key = entry_section(&self.entries[from]);
        let to = match before {
            Some(before) => match self.entries.iter().position(|e| matches!(e, Entry::Bookmark(b) if b.id == before)) {
                Some(to) if entry_section(&self.entries[to]) == key => to,
                _ => return false,
            },
            None => match self.entries.iter().rposition(|e| entry_section(e) == key) {
                Some(to) => to,
                None => return false,
            },
        };
        if from == to {
            return false;
        }
        let entry = self.entries.remove(from);
        self.entries.insert(to, entry);
        true
    }

    /// Drags a whole section (a file's bookmarks or a search group) to
    /// where section `before` is.
    pub fn move_section(&mut self, key: &SectionKey, before: &SectionKey) -> bool {
        if key == before {
            return false;
        }
        let order: Vec<SectionKey> = self.sections().into_iter().map(|s| s.key).collect();
        let (Some(from), Some(to)) = (order.iter().position(|k| k == key), order.iter().position(|k| k == before)) else {
            return false;
        };
        let (moved, mut rest): (Vec<Entry>, Vec<Entry>) =
            std::mem::take(&mut self.entries).into_iter().partition(|e| entry_section(e) == *key);
        // Dragged down, it lands after the target; up, before it.
        let anchor = if from < to { order.get(to + 1) } else { Some(before) };
        let at = anchor.and_then(|k| rest.iter().position(|e| entry_section(e) == *k)).unwrap_or(rest.len());
        rest.splice(at..at, moved);
        self.entries = rest;
        true
    }

    /// Sort By: sections and the bookmarks in each. Stable, so equal ones
    /// keep their order.
    pub fn sort(&mut self, by: SortBy) {
        let name_of = |entry: &Entry| match entry {
            Entry::Bookmark(b) => (file_name(b.location.path()).to_lowercase(), b.display_title().to_lowercase()),
            Entry::Search(group) => (group.display_title().to_lowercase(), String::new()),
        };
        let kind_of = |entry: &Entry| match entry {
            Entry::Bookmark(Bookmark { location: Location::File { .. }, .. }) => 0,
            Entry::Bookmark(_) => 1,
            Entry::Search(_) => 2,
        };
        let done_of = |entry: &Entry| matches!(entry, Entry::Bookmark(b) if b.completed);
        match by {
            SortBy::Name => self.entries.sort_by_cached_key(name_of),
            SortBy::Type => self.entries.sort_by_key(kind_of),
            SortBy::CompletionStatus => self.entries.sort_by_key(done_of),
        }
        for entry in &mut self.entries {
            if let Entry::Search(group) = entry {
                match by {
                    SortBy::Name => group.results.sort_by_cached_key(|b| (file_name(b.location.path()).to_lowercase(), b.location.line())),
                    SortBy::Type => {}
                    SortBy::CompletionStatus => group.results.sort_by_key(|b| b.completed),
                }
            }
        }
    }

    /// Jump to Next / Previous Bookmark: the nearest bookmarked line after
    /// (or before) `line` in `path`, wrapping around the file.
    pub fn step_in_file(&self, path: &Path, line: u32, forward: bool) -> Option<u32> {
        let mut lines: Vec<u32> = self.lines_in(path).map(|(_, l)| l).collect();
        lines.sort_unstable();
        lines.dedup();
        if forward {
            lines.iter().find(|&&l| l > line).or(lines.first()).copied()
        } else {
            lines.iter().rev().find(|&&l| l < line).or(lines.last()).copied()
        }
    }

    /// Jump to Next / Previous Bookmark In Workspace: the bookmark after
    /// (or before) `current` (the one last jumped to, while the cursor is
    /// still there) or the one at `line`, in the navigator's order. Away
    /// from a bookmark, the nearest one in the file after (or before) the
    /// line, or else the first (or last) bookmark.
    pub fn step_in_workspace(&self, path: &Path, line: u32, current: Option<u64>, forward: bool) -> Option<&Bookmark> {
        let order = self.in_order();
        if order.is_empty() {
            return None;
        }
        let at = |b: &Bookmark| b.location.path() == path && b.location.line() == Some(line);
        let current = current.and_then(|id| order.iter().position(|b| b.id == id));
        if let Some(ix) = current.or_else(|| order.iter().position(|b| at(b))) {
            let len = order.len();
            let next = if forward { (ix + 1) % len } else { (ix + len - 1) % len };
            return Some(order[next]);
        }
        let in_file = order.iter().filter(|b| b.location.path() == path).filter_map(|b| Some((b.location.line()?, *b)));
        let nearest = if forward {
            in_file.filter(|(l, _)| *l > line).min_by_key(|(l, _)| *l)
        } else {
            in_file.filter(|(l, _)| *l < line).max_by_key(|(l, _)| *l)
        };
        nearest.map(|(_, b)| b).or(if forward { order.first() } else { order.last() }.copied())
    }
}

fn entry_section(entry: &Entry) -> SectionKey {
    match entry {
        Entry::Bookmark(b) => SectionKey::File(b.location.path().to_path_buf()),
        Entry::Search(group) => SectionKey::Search(group.id),
    }
}

/// Whether a bookmark shows with the navigator's filter: its description,
/// file name or line number contains the text (ignoring case).
pub fn matches_filter(bookmark: &Bookmark, filter: &str) -> bool {
    let filter = filter.trim().to_lowercase();
    if filter.is_empty() {
        return true;
    }
    bookmark.display_title().to_lowercase().contains(&filter)
        || file_name(bookmark.location.path()).to_lowercase().contains(&filter)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(path: &str, line: u32) -> Location {
        Location::Line { path: path.into(), line }
    }

    fn titles(bookmarks: &Bookmarks) -> Vec<String> {
        bookmarks.in_order().iter().map(|b| b.display_title()).collect()
    }

    #[test]
    fn adding_twice_keeps_one_and_untitled_ones_say_where_they_are() {
        let mut b = Bookmarks::default();
        let (id, added) = b.add(line("/p/a.swift", 3), "let x = 1");
        assert!(added);
        assert_eq!(b.add(line("/p/a.swift", 3), "again"), (id, false));
        b.add(Location::File { path: "/p/b.swift".into() }, "");
        assert_eq!(titles(&b), ["let x = 1", "b.swift"]);
        b.set_title(id, "  ");
        assert_eq!(b.get(id).unwrap().display_title(), "a.swift:3");
    }

    #[test]
    fn sections_group_by_file_in_first_appearance_order() {
        let mut b = Bookmarks::default();
        b.add(line("/p/a.swift", 9), "a9");
        b.add(line("/p/b.swift", 1), "b1");
        b.add(line("/p/a.swift", 2), "a2");
        let search = b.add_search("tick".into(), SearchOptions::default(), vec![("/p/c.swift".into(), 4, " tick() ".into())]);
        let sections = b.sections();
        assert_eq!(
            sections.iter().map(|s| (s.key.clone(), s.bookmarks.len())).collect::<Vec<_>>(),
            vec![(SectionKey::File("/p/a.swift".into()), 2), (SectionKey::File("/p/b.swift".into()), 1), (SectionKey::Search(search), 1)]
        );
        assert_eq!(sections[2].title(), "“tick”");
        assert_eq!(titles(&b), ["a9", "a2", "b1", "tick()"]);
    }

    #[test]
    fn dragging_reorders_within_a_section_and_moves_sections() {
        let mut b = Bookmarks::default();
        let a9 = b.add(line("/p/a.swift", 9), "a9").0;
        let b1 = b.add(line("/p/b.swift", 1), "b1").0;
        let a2 = b.add(line("/p/a.swift", 2), "a2").0;
        // Not across files.
        assert!(!b.move_bookmark(a2, Some(b1)));
        assert!(b.move_bookmark(a2, Some(a9)));
        assert_eq!(titles(&b), ["a2", "a9", "b1"]);
        assert!(b.move_bookmark(a2, None));
        assert_eq!(titles(&b), ["a9", "a2", "b1"]);
        let (fa, fb) = (SectionKey::File("/p/a.swift".into()), SectionKey::File("/p/b.swift".into()));
        assert!(b.move_section(&fb, &fa));
        assert_eq!(titles(&b), ["b1", "a9", "a2"]);
        assert!(b.move_section(&fb, &fa));
        assert_eq!(titles(&b), ["a9", "a2", "b1"]);
        let search = b.add_search("x".into(), SearchOptions::default(), vec![("/p/c.swift".into(), 1, "x1".into()), ("/p/c.swift".into(), 2, "x2".into())]);
        assert!(b.move_section(&SectionKey::Search(search), &fa));
        assert_eq!(titles(&b), ["x1", "x2", "a9", "a2", "b1"]);
        let x2 = b.search(search).unwrap().results[1].id;
        assert!(b.move_bookmark(x2, Some(b.search(search).unwrap().results[0].id)));
        assert_eq!(titles(&b), ["x2", "x1", "a9", "a2", "b1"]);
    }

    #[test]
    fn edits_move_and_delete_line_bookmarks() {
        let mut b = Bookmarks::default();
        let first = b.add(line("/p/a.swift", 3), "three").0;
        let second = b.add(line("/p/a.swift", 5), "five").0;
        let search = b.add_search("q".into(), SearchOptions::default(), vec![("/p/a.swift".into(), 5, "five".into())]);
        let result = b.search(search).unwrap().results[0].id;
        b.move_lines(Path::new("/p/a.swift"), &[(first, Some(4)), (second, Some(6)), (result, Some(6))]);
        assert_eq!(b.lines_in(Path::new("/p/a.swift")).collect::<Vec<_>>(), vec![(first, 4), (second, 6), (result, 6)]);
        // Joined lines keep the first bookmark.
        b.move_lines(Path::new("/p/a.swift"), &[(second, Some(4))]);
        assert_eq!(b.lines_in(Path::new("/p/a.swift")).map(|(id, _)| id).collect::<Vec<_>>(), vec![first, result]);
        b.move_lines(Path::new("/p/a.swift"), &[(result, None)]);
        assert!(b.search(search).unwrap().results.is_empty());
    }

    #[test]
    fn renames_and_trash_follow_files_and_folders() {
        let mut b = Bookmarks::default();
        b.add(line("/p/Sources/a.swift", 1), "a");
        b.add(Location::File { path: "/p/Sources/b.swift".into() }, "b");
        b.add(line("/p/other.swift", 1), "o");
        assert!(b.move_files(Path::new("/p/Sources"), Path::new("/p/Lib")));
        assert_eq!(
            b.all().map(|b| b.location.path().to_path_buf()).collect::<Vec<_>>(),
            [PathBuf::from("/p/Lib/a.swift"), "/p/Lib/b.swift".into(), "/p/other.swift".into()]
        );
        assert!(b.remove_in(Path::new("/p/Lib")));
        assert_eq!(titles(&b), ["o"]);
    }

    #[test]
    fn find_again_keeps_descriptions_and_completion() {
        let mut b = Bookmarks::default();
        let results = vec![("/p/a.swift".into(), 1, "a".into()), ("/p/a.swift".into(), 1, "a again".into()), ("/p/b.swift".into(), 2, "b".into())];
        let id = b.add_search("q".into(), SearchOptions { case_sensitive: true, ..Default::default() }, results);
        assert_eq!(b.search(id).unwrap().results.len(), 2, "one per line");
        let first = b.search(id).unwrap().results[0].id;
        b.set_title(first, "renamed");
        b.set_completed(first, true);
        b.replace_search_results(id, vec![("/p/a.swift".into(), 1, "a".into()), ("/p/c.swift".into(), 3, "c".into())]);
        let group = b.search(id).unwrap();
        assert_eq!(group.results.iter().map(|b| (b.display_title(), b.completed)).collect::<Vec<_>>(), [("renamed".into(), true), ("c".into(), false)]);
        b.set_title(id, "q");
        assert_eq!(b.search(id).unwrap().title, "", "the query is the default description");
        assert_eq!(b.remove_completed(), 1);
        assert_eq!(b.search(id).unwrap().results.len(), 1);
        assert!(b.remove(id));
        assert!(b.is_empty());
    }

    #[test]
    fn sorting() {
        let mut b = Bookmarks::default();
        let zeta = b.add(line("/p/z.swift", 1), "zeta").0;
        b.add(Location::File { path: "/p/m.swift".into() }, "m");
        b.add(line("/p/a.swift", 1), "beta");
        b.add(line("/p/a.swift", 2), "Alpha");
        b.sort(SortBy::Name);
        assert_eq!(titles(&b), ["Alpha", "beta", "m", "zeta"]);
        b.sort(SortBy::Type);
        assert_eq!(titles(&b), ["m", "Alpha", "beta", "zeta"]);
        b.set_completed(zeta, true);
        b.sort(SortBy::Name);
        b.sort(SortBy::CompletionStatus);
        assert_eq!(titles(&b), ["Alpha", "beta", "m", "zeta"]);
    }

    #[test]
    fn next_and_previous() {
        let mut b = Bookmarks::default();
        let a = Path::new("/p/a.swift");
        b.add(line("/p/a.swift", 10), "a10");
        b.add(line("/p/b.swift", 4), "b4");
        b.add(line("/p/a.swift", 3), "a3");
        assert_eq!(b.step_in_file(a, 1, true), Some(3));
        assert_eq!(b.step_in_file(a, 3, true), Some(10));
        assert_eq!(b.step_in_file(a, 10, true), Some(3), "wraps");
        assert_eq!(b.step_in_file(a, 3, false), Some(10));
        assert_eq!(b.step_in_file(Path::new("/p/none.swift"), 1, true), None);
        // In the navigator's order: a10, a3, b4.
        let step = |path: &str, line: u32, forward: bool| b.step_in_workspace(Path::new(path), line, None, forward).map(|b| b.display_title());
        assert_eq!(step("/p/a.swift", 10, true).as_deref(), Some("a3"));
        assert_eq!(step("/p/a.swift", 3, true).as_deref(), Some("b4"));
        assert_eq!(step("/p/b.swift", 4, true).as_deref(), Some("a10"), "wraps");
        assert_eq!(step("/p/a.swift", 10, false).as_deref(), Some("b4"));
        assert_eq!(step("/p/a.swift", 5, true).as_deref(), Some("a10"), "nearest below the cursor");
        assert_eq!(step("/p/c.swift", 1, false).as_deref(), Some("b4"));
        // From a file bookmark, which has no line to be at.
        let file = b.add(Location::File { path: "/p/a.swift".into() }, "a").0;
        assert_eq!(b.step_in_workspace(a, 3, Some(file), true).map(|b| b.display_title()).as_deref(), Some("b4"));
    }

    #[test]
    fn saved_bookmarks_round_trip() {
        let mut b = Bookmarks::default();
        b.add(line("/p/a.swift", 3), "three");
        let file = b.add(Location::File { path: "/p/b.swift".into() }, "b.swift").0;
        b.set_completed(file, true);
        b.add_search("tick".into(), SearchOptions { regex: true, ..Default::default() }, vec![("/p/c.swift".into(), 7, "tick".into())]);
        let saved = serde_json::to_value(&b.entries).unwrap();
        assert_eq!(
            saved[0],
            serde_json::json!({ "entry": "bookmark", "location": { "type": "line", "path": "/p/a.swift", "line": 3 }, "title": "three" })
        );
        let mut restored = Bookmarks::default();
        restored.restore(serde_json::from_value(saved).unwrap());
        assert_eq!(titles(&restored), titles(&b));
        assert!(restored.get(restored.in_order()[1].id).unwrap().completed);
        assert!(restored.search(restored.sections()[2].search.unwrap().id).unwrap().options.regex);
    }

    #[test]
    fn filtering() {
        let bookmark = Bookmark::new(line("/p/Counter.swift", 3), "count += 1");
        assert!(matches_filter(&bookmark, ""));
        assert!(matches_filter(&bookmark, "COUNT +"));
        assert!(matches_filter(&bookmark, "counter.sw"));
        assert!(!matches_filter(&bookmark, "reset"));
    }
}

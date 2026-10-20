//! Find in Workspace: a parallel, gitignore-aware text search.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use regex::{Regex, RegexBuilder};

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

#[derive(Clone, Debug)]
pub struct LineMatch {
    /// 0-based line.
    pub line: u32,
    /// UTF-16 column of the match start in the original line.
    pub column: u32,
    /// Byte range of the match within `preview`.
    pub range: std::ops::Range<usize>,
    pub preview: String,
}

#[derive(Clone, Debug)]
pub struct FileMatches {
    pub path: PathBuf,
    pub matches: Vec<LineMatch>,
}

pub const MAX_MATCHES: usize = 5_000;
const MAX_FILE_SIZE: u64 = 4 * 1024 * 1024;
const MAX_PREVIEW: usize = 240;

pub fn build_regex(query: &str, options: &SearchOptions) -> Result<Regex, regex::Error> {
    let mut pattern = if options.regex {
        query.to_string()
    } else {
        regex::escape(query)
    };
    if options.whole_word {
        pattern = format!(r"\b{pattern}\b");
    }
    RegexBuilder::new(&pattern)
        .case_insensitive(!options.case_sensitive)
        .build()
}

/// Searches `files` (relative to `root`), the text in `unsaved` (by
/// absolute path) instead of the file for those with unsaved edits.
/// Blocking; checks `cancel` between files so a newer query can abort an
/// older one.
pub fn search(
    root: &Path,
    files: &[PathBuf],
    unsaved: &HashMap<PathBuf, String>,
    regex: &Regex,
    cancel: &AtomicBool,
) -> Vec<FileMatches> {
    let total = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8);
    let chunk = files.len().div_ceil(threads).max(1);
    let results: Vec<Vec<(usize, FileMatches)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .enumerate()
            .map(|(chunk_ix, chunk_files)| {
                let total = &total;
                scope.spawn(move || {
                    let mut out = Vec::new();
                    for (i, rel) in chunk_files.iter().enumerate() {
                        if cancel.load(Ordering::Relaxed) || total.load(Ordering::Relaxed) >= MAX_MATCHES {
                            break;
                        }
                        let path = root.join(rel);
                        let found = match unsaved.get(&path) {
                            Some(text) => search_text(&path, text, regex),
                            None => search_file(&path, regex),
                        };
                        if let Some(found) = found {
                            total.fetch_add(found.matches.len(), Ordering::Relaxed);
                            out.push((chunk_ix * chunk + i, found));
                        }
                    }
                    out
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
    });
    let mut all: Vec<_> = results.into_iter().flatten().collect();
    all.sort_by_key(|(ix, _)| *ix);
    all.into_iter().map(|(_, m)| m).collect()
}

fn search_file(path: &Path, regex: &Regex) -> Option<FileMatches> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FILE_SIZE {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return None;
    }
    search_text(path, &String::from_utf8_lossy(&bytes), regex)
}

fn search_text(path: &Path, text: &str, regex: &Regex) -> Option<FileMatches> {
    let mut matches = Vec::new();
    for (line_ix, line) in text.lines().enumerate() {
        for m in regex.find_iter(line) {
            let (preview, range) = preview(line, m.start()..m.end());
            matches.push(LineMatch {
                line: line_ix as u32,
                column: line[..m.start()].encode_utf16().count() as u32,
                range,
                preview,
            });
            if matches.len() >= 200 {
                break;
            }
        }
    }
    (!matches.is_empty()).then(|| FileMatches {
        path: path.to_path_buf(),
        matches,
    })
}

/// Trims long lines around the match, keeping indices valid.
fn preview(line: &str, range: std::ops::Range<usize>) -> (String, std::ops::Range<usize>) {
    let trimmed_start = line.len() - line.trim_start().len();
    let mut start = trimmed_start.min(range.start);
    if range.start - start > MAX_PREVIEW / 2 {
        start = range.start - MAX_PREVIEW / 2;
        while !line.is_char_boundary(start) {
            start += 1;
        }
    }
    let mut end = line.len().min(start + MAX_PREVIEW).max(range.end);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    (
        line[start..end].to_string(),
        range.start - start..range.end - start,
    )
}

/// A preview shortened at its start, so the match shows in a row only a
/// few dozen characters wide: at most `lead` characters before the match,
/// after an ellipsis (as Xcode's Find navigator shows them).
pub fn elide_start(preview: &str, range: std::ops::Range<usize>, lead: usize) -> (String, std::ops::Range<usize>) {
    let before = &preview[..range.start];
    let Some((cut, _)) = before.char_indices().rev().nth(lead) else {
        return (preview.to_string(), range);
    };
    // After the character `lead + 1` back, and any spaces there.
    let mut start = cut + before[cut..].chars().next().map_or(0, char::len_utf8);
    start += preview[start..range.start].len() - preview[start..range.start].trim_start().len();
    let shown = format!("…{}", &preview[start..]);
    let shift = '…'.len_utf8();
    (shown, range.start - start + shift..range.end - start + shift)
}

pub type CancelFlag = Arc<AtomicBool>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_matches() {
        let dir = std::env::temp_dir().join(format!("zcode-search-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.swift"), "let counter = 1\n    counter += 1\n").unwrap();
        std::fs::write(dir.join("b.swift"), "nothing here\n").unwrap();
        let regex = build_regex("Counter", &SearchOptions::default()).unwrap();
        let files = vec![PathBuf::from("a.swift"), PathBuf::from("b.swift")];
        let none = HashMap::new();
        let results = search(&dir, &files, &none, &regex, &AtomicBool::new(false));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].matches.len(), 2);
        assert_eq!(results[0].matches[1].line, 1);
        assert_eq!(&results[0].matches[1].preview[results[0].matches[1].range.clone()], "counter");
        let strict = build_regex("Counter", &SearchOptions { case_sensitive: true, ..Default::default() }).unwrap();
        assert!(search(&dir, &files, &none, &strict, &AtomicBool::new(false)).is_empty());
        // A file with unsaved edits: its text, not the file.
        let unsaved = HashMap::from([(dir.join("a.swift"), "// three lines\n// up\nlet counter = 1\n".to_string()), (dir.join("b.swift"), "counter\n".to_string())]);
        let results = search(&dir, &files, &unsaved, &regex, &AtomicBool::new(false));
        let found: Vec<(&str, Vec<u32>)> = results
            .iter()
            .map(|f| (f.path.file_name().unwrap().to_str().unwrap(), f.matches.iter().map(|m| m.line).collect()))
            .collect();
        assert_eq!(found, vec![("a.swift", vec![2]), ("b.swift", vec![0])]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn long_previews_elide_their_start_to_show_the_match() {
        let shown = |preview: &str, needle: &str, lead: usize| {
            let start = preview.find(needle).unwrap();
            let (text, range) = elide_start(preview, start..start + needle.len(), lead);
            assert_eq!(&text[range.clone()], needle);
            text
        };
        assert_eq!(shown("let short = e", "e", 10), "let short = e");
        assert_eq!(shown(".package(url: \"https://example.com/x\", from: \"1.0.0\")", "from", 8), "…com/x\", from: \"1.0.0\")");
        // Never starts on a space, nor inside a character.
        assert_eq!(shown("aaaaaaaa      bé match", "match", 6), "…bé match");
        assert_eq!(shown("ééééééééééé match", "match", 3), "…éé match");
    }
}

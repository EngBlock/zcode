//! Find in Workspace: a parallel, gitignore-aware text search.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use regex::{Regex, RegexBuilder};

#[derive(Clone, Debug, Default, PartialEq)]
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

/// Searches `files` (relative to `root`). Blocking; checks `cancel` between
/// files so a newer query can abort an older one.
pub fn search(
    root: &Path,
    files: &[PathBuf],
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
                        if let Some(found) = search_file(&path, regex) {
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
    let text = String::from_utf8_lossy(&bytes);
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
        let results = search(&dir, &files, &regex, &AtomicBool::new(false));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].matches.len(), 2);
        assert_eq!(results[0].matches[1].line, 1);
        assert_eq!(&results[0].matches[1].preview[results[0].matches[1].range.clone()], "counter");
        let strict = build_regex("Counter", &SearchOptions { case_sensitive: true, ..Default::default() }).unwrap();
        assert!(search(&dir, &files, &strict, &AtomicBool::new(false)).is_empty());
        std::fs::remove_dir_all(dir).ok();
    }
}

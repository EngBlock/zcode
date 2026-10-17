//! Links in a terminal's text: URLs, which open in the browser, and file
//! paths with an optional `:line[:column]` (as compilers, `grep -n` and
//! test runners print them), which open in the editor. A path is a link
//! only when the file exists, relative to the terminal's directory.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Url(String),
    File { path: PathBuf, line: Option<u32>, column: Option<u32> },
}

static URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?:https?|file|ftp|ssh|git|mailto)://[^\s<>"'`{}|\\^\[\]]+"#).unwrap());

/// A path (at least one `/` or `.` in it, so plain words aren't), then
/// `:line` and `:column` when they follow.
static PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\w~.@%+\-/]*[\w~@%+\-][\w~.@%+\-/]*(?::(\d+))?(?::(\d+))?").unwrap());

/// The link under character `at` of `text` (a line as shown), with the
/// characters it spans; `dir` is where relative paths start.
pub fn link_at(text: &str, at: usize, dir: &Path, home: Option<&Path>) -> Option<(Range<usize>, Target)> {
    let byte_at = text.char_indices().nth(at).map(|(ix, _)| ix)?;
    let chars = |range: Range<usize>| text[..range.start].chars().count()..text[..range.end].chars().count();
    for found in URL.find_iter(text) {
        let url = trim_url(found.as_str());
        let range = found.start()..found.start() + url.len();
        if range.contains(&byte_at) {
            let target = match url.strip_prefix("file://") {
                Some(path) => Target::File { path: PathBuf::from(path), line: None, column: None },
                None => Target::Url(url.to_string()),
            };
            return Some((chars(range), target));
        }
    }
    for found in PATH.captures_iter(text) {
        let whole = found.get(0)?;
        if !(whole.start()..whole.end()).contains(&byte_at) {
            continue;
        }
        let path_end = found.get(1).map_or(whole.end(), |line| line.start() - 1);
        // A sentence's full stop or a quote's colon isn't the path's.
        let path_text = whole.as_str()[..path_end - whole.start()].trim_end_matches(['.', ':']);
        if !path_text.contains(['/', '.']) {
            return None;
        }
        let path = resolve(path_text, dir, home)?;
        let line = found.get(1).and_then(|m| m.as_str().parse().ok());
        let column = found.get(2).and_then(|m| m.as_str().parse().ok());
        let end = if line.is_some() { whole.end() } else { whole.start() + path_text.len() };
        return Some((chars(whole.start()..end), Target::File { path, line, column }));
    }
    None
}

/// An existing file `text` names, relative to `dir`.
fn resolve(text: &str, dir: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let mut candidates = vec![];
    match text.strip_prefix("~/") {
        Some(rest) => candidates.push(home?.join(rest)),
        None => {
            candidates.push(dir.join(text));
            // `git diff`'s a/ and b/ sides.
            candidates.extend(text.strip_prefix("a/").or_else(|| text.strip_prefix("b/")).map(|rest| dir.join(rest)));
        }
    }
    let path = candidates.into_iter().find(|path| path.is_file())?;
    Some(path.canonicalize().unwrap_or(path))
}

/// A URL without the punctuation after it in a sentence, or the closing
/// parenthesis around it.
fn trim_url(url: &str) -> &str {
    let mut url = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '"']);
    while url.ends_with(')') && url.matches('(').count() < url.matches(')').count() {
        url = &url[..url.len() - 1];
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zcode-links-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Sources/App")).unwrap();
        std::fs::write(dir.join("Sources/App/main.swift"), "print(1)\n").unwrap();
        std::fs::write(dir.join("Package.swift"), "// swift-tools-version:5.9\n").unwrap();
        dir.canonicalize().unwrap()
    }

    fn file(path: &Path, line: Option<u32>, column: Option<u32>) -> Target {
        Target::File { path: path.to_path_buf(), line, column }
    }

    #[test]
    fn urls_without_the_sentence_around_them() {
        let text = "see (https://example.com/a_(b)/c?d=1), then";
        let at = text.find("example").unwrap();
        let (range, target) = link_at(text, at, Path::new("/"), None).unwrap();
        assert_eq!(target, Target::Url("https://example.com/a_(b)/c?d=1".into()));
        assert_eq!(&text[range], "https://example.com/a_(b)/c?d=1");
    }

    #[test]
    fn compiler_style_paths_with_lines_and_columns() {
        let dir = dir();
        let main = dir.join("Sources/App/main.swift");
        let text = "Sources/App/main.swift:12:5: error: cannot find 'x' in scope";
        let (range, target) = link_at(text, 3, &dir, None).unwrap();
        assert_eq!(target, file(&main, Some(12), Some(5)));
        assert_eq!(range, 0..27);
        // Absolute, line only, at the end of a sentence.
        let text = format!("failed at {}:7.", main.display());
        let at = text.find("main").unwrap();
        assert_eq!(link_at(&text, at, Path::new("/"), None).unwrap().1, file(&main, Some(7), None));
        // A file without a line; `git diff`'s a/ and b/ prefixes.
        assert_eq!(link_at("open Package.swift.", 7, &dir, None).unwrap(), (5..18, file(&dir.join("Package.swift"), None, None)));
        assert_eq!(link_at("+++ b/Package.swift", 8, &dir, None).unwrap().1, file(&dir.join("Package.swift"), None, None));
        // ~ is home.
        assert_eq!(link_at("~/Package.swift:3", 4, Path::new("/"), Some(&dir)).unwrap().1, file(&dir.join("Package.swift"), Some(3), None));
    }

    #[test]
    fn missing_files_and_plain_words_are_not_links() {
        let dir = dir();
        assert_eq!(link_at("Sources/App/gone.swift:3", 2, &dir, None), None);
        assert_eq!(link_at("hello world", 2, &dir, None), None);
        assert_eq!(link_at("Sources/App/main.swift and more", 25, &dir, None), None);
    }
}

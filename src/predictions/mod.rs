//! Predictive code completion (roadmap 3.3): what to ask a provider and
//! what to make of its answer. Two providers: the Claude API (`claude.rs`),
//! which streams a prediction from the code around the cursor and the
//! edits made lately, and a language server's
//! `textDocument/inlineCompletion` (`lsp.rs`), when it offers one. The
//! window side (debouncing, cancelling, showing, the status bar) is
//! `crate::workspace::predictions`; showing and accepting is the editor's
//! (`zcode_editor::EditPrediction`).

pub mod claude;
pub mod lsp;
pub mod sse;

use std::collections::VecDeque;
use std::ops::Range;
use std::path::{Path, PathBuf};

use zcode_editor::{Rope, RopeExt as _};

/// How much of the file before and after the cursor a request carries.
const CONTEXT_BEFORE: usize = 6_000;
const CONTEXT_AFTER: usize = 2_000;
/// How many recent edits a request carries, and how much of each.
const MAX_RECENT_EDITS: usize = 8;
const MAX_EDIT_TEXT: usize = 1_000;

/// What a provider is asked about.
#[derive(Clone, Debug)]
pub struct PredictionRequest {
    pub path: PathBuf,
    /// The path shown to the model: relative to the project.
    pub display_path: String,
    pub language: String,
    pub text: Rope,
    pub cursor: usize,
    pub recent_edits: Vec<RecentEdit>,
    /// Asked for (Show Predictive Completion) rather than typed into.
    pub invoked: bool,
}

/// What a provider has predicted so far.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    /// The whole prediction so far, to insert at the request's cursor (the
    /// Claude API, streaming).
    Text(String),
    /// Text in place of a range (a language server's item).
    Replace(Range<usize>, String),
    Done,
    Failed(String),
}

/// The text around `cursor`: up to `CONTEXT_BEFORE` bytes before it and
/// `CONTEXT_AFTER` after, cut at line boundaries, and the 0-based row the
/// excerpt starts on.
pub fn excerpt(text: &Rope, cursor: usize) -> (String, String, usize) {
    let cursor = cursor.min(text.len());
    let start_row = text.offset_to_point(cursor.saturating_sub(CONTEXT_BEFORE)).row;
    // A line cut in half reads as broken code: start at the next one, unless
    // the cut is on the cursor's own line.
    let cursor_row = text.offset_to_point(cursor).row;
    let start_row = if start_row < cursor_row && cursor.saturating_sub(CONTEXT_BEFORE) > text.line_start_offset(start_row) {
        start_row + 1
    } else {
        start_row
    };
    let start = text.line_start_offset(start_row).min(cursor);
    let end_row = text.offset_to_point((cursor + CONTEXT_AFTER).min(text.len())).row;
    let end = if end_row > cursor_row && (cursor + CONTEXT_AFTER) < text.len() {
        text.line_start_offset(end_row)
    } else {
        text.line_end_offset(end_row).max(cursor)
    };
    (text.slice(start..cursor).to_string(), text.slice(cursor..end.max(cursor)).to_string(), start_row)
}

/// A prediction made ready to insert: what repeats the rest of the cursor's
/// line (the `)` a closed bracket already typed) taken off its end, and
/// nothing at all when it's only whitespace.
pub fn clean(prediction: &str, rest_of_line: &str) -> Option<String> {
    if prediction.trim().is_empty() {
        return None;
    }
    let rest = rest_of_line.trim_end();
    let mut text = prediction.trim_end_matches(['\n', '\r']).to_string();
    if !rest.is_empty() {
        // The longest end of the prediction that starts the rest of the line.
        let overlap = (1..=rest.len().min(text.len()))
            .rev()
            .filter(|n| rest.is_char_boundary(*n) && text.is_char_boundary(text.len() - n))
            .find(|n| text.ends_with(&rest[..*n]));
        if let Some(n) = overlap {
            text.truncate(text.len() - n);
        }
    }
    (!text.trim().is_empty()).then_some(text)
}

/// Whether `path` matches one of `globs` (`edit_predictions.disabled_globs`),
/// relative to the project or as a whole path.
pub fn disabled(path: &Path, root: &Path, globs: &[String]) -> bool {
    let mut set = globset::GlobSetBuilder::new();
    for glob in globs {
        if let Ok(glob) = globset::GlobBuilder::new(glob).literal_separator(false).build() {
            set.add(glob);
        }
    }
    let Ok(set) = set.build() else { return false };
    let relative = path.strip_prefix(root).unwrap_or(path);
    set.is_match(relative) || set.is_match(path)
}

/// An edit made lately, as the lines it changed.
#[derive(Clone, Debug, PartialEq)]
pub struct RecentEdit {
    pub path: String,
    /// The 0-based row its lines start on, after it.
    pub row: usize,
    /// How many lines it covers now.
    pub rows: usize,
    pub old: String,
    pub new: String,
}

/// The last few edits across files, typing coalesced into one edit per
/// place, so a request can say what the user has been doing.
#[derive(Default)]
pub struct RecentEdits {
    edits: VecDeque<RecentEdit>,
}

impl RecentEdits {
    /// `range` of `before` (in `path`) replaced, giving `after`.
    pub fn record(&mut self, path: &str, before: &Rope, range: Range<usize>, after: &Rope, new_len: usize) {
        let row = before.offset_to_point(range.start).row;
        let old_end_row = before.offset_to_point(range.end.min(before.len())).row;
        let new_end_row = after.offset_to_point((range.start + new_len).min(after.len())).row;
        let lines = |text: &Rope, first: usize, last: usize| -> String {
            let start = text.line_start_offset(first);
            let end = text.line_end_offset(last.min(text.lines_len().saturating_sub(1)));
            text.slice(start..end.max(start)).to_string()
        };
        // Continuing the last edit (typing on, deleting what was typed):
        // one edit with the old lines it started from.
        if let Some(last) = self.edits.back_mut()
            && last.path == path
            && row >= last.row
            && old_end_row < last.row + last.rows
        {
            let added = new_end_row as isize - old_end_row as isize;
            last.rows = (last.rows as isize + added).max(1) as usize;
            last.new = truncate(lines(after, last.row, last.row + last.rows - 1));
            if last.old == last.new {
                self.edits.pop_back();
            }
            return;
        }
        let edit = RecentEdit {
            path: path.to_string(),
            row,
            rows: new_end_row - row + 1,
            old: truncate(lines(before, row, old_end_row)),
            new: truncate(lines(after, row, new_end_row)),
        };
        if edit.old != edit.new {
            self.edits.push_back(edit);
            while self.edits.len() > MAX_RECENT_EDITS {
                self.edits.pop_front();
            }
        }
    }

    pub fn list(&self) -> Vec<RecentEdit> {
        self.edits.iter().cloned().collect()
    }

    pub fn clear(&mut self) {
        self.edits.clear();
    }
}

fn truncate(mut text: String) -> String {
    if text.len() > MAX_EDIT_TEXT {
        let mut end = MAX_EDIT_TEXT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_excerpt_is_whole_lines_around_the_cursor() {
        let text = Rope::from("a\nbb\nccc\n");
        assert_eq!(excerpt(&text, 4), ("a\nbb".into(), "\nccc\n".into(), 0));
        let long: String = (0..2000).map(|i| format!("line {i}\n")).collect();
        let text = Rope::from(long.as_str());
        let cursor = text.line_start_offset(1500) + 2;
        let (before, after, row) = excerpt(&text, cursor);
        assert!(before.len() <= CONTEXT_BEFORE && before.starts_with("line ") && before.ends_with("\nli"), "{before:?}");
        assert_eq!(row, 1500 - before.matches('\n').count());
        assert!(after.len() <= CONTEXT_AFTER + 20 && after.starts_with("ne 1500\n") && after.ends_with('\n'));
    }

    #[test]
    fn cleaning_drops_what_the_line_already_has() {
        assert_eq!(clean("name: String)", ")"), Some("name: String".into()));
        assert_eq!(clean("x) {", ") {"), Some("x".into()));
        assert_eq!(clean("print(x)\n", ""), Some("print(x)".into()));
        assert_eq!(clean("  \n", ""), None);
        assert_eq!(clean(")", ")"), None);
        assert_eq!(clean("a + b", "\n"), Some("a + b".into()));
    }

    #[test]
    fn secrets_and_keys_are_never_sent() {
        let globs: Vec<String> = crate::settings::EditPredictionSettings::default().disabled_globs;
        let root = Path::new("/p");
        assert!(disabled(Path::new("/p/.env"), root, &globs));
        assert!(disabled(Path::new("/p/config/.env.local"), root, &globs));
        assert!(disabled(Path::new("/p/certs/dev.pem"), root, &globs));
        assert!(!disabled(Path::new("/p/Sources/App.swift"), root, &globs));
    }

    #[test]
    fn typing_coalesces_into_one_edit() {
        let mut edits = RecentEdits::default();
        let mut text = Rope::from("let a = 1\nlet b = 2\n");
        let type_at = |edits: &mut RecentEdits, text: &mut Rope, at: usize, s: &str| {
            let before = text.clone();
            text.replace(at..at, s);
            edits.record("A.swift", &before, at..at, text, s.len());
        };
        type_at(&mut edits, &mut text, 9, "0");
        type_at(&mut edits, &mut text, 10, "0");
        assert_eq!(edits.list(), [RecentEdit { path: "A.swift".into(), row: 0, rows: 1, old: "let a = 1".into(), new: "let a = 100".into() }]);
        // A new line typed in the same place stays one edit.
        type_at(&mut edits, &mut text, 11, "\nlet c = 3");
        assert_eq!(edits.list().len(), 1);
        assert_eq!(edits.list()[0].new, "let a = 100\nlet c = 3");
        // Elsewhere is another.
        let at = text.line_start_offset(2) + 4;
        type_at(&mut edits, &mut text, at, "_");
        assert_eq!(edits.list().len(), 2);
        // Deleting what was typed leaves nothing to say.
        let before = text.clone();
        text.replace(at..at + 1, "");
        edits.record("A.swift", &before, at..at + 1, &text, 0);
        assert_eq!(edits.list().len(), 1);
    }
}

//! Line diffs between two versions of a file, as hunks: what the gutter's
//! change bars, the diff view and the commit sheet show, and what staging,
//! unstaging and discarding a hunk apply.
//!
//! Hunks have no context lines: each is a run of changed lines, old rows
//! against new rows (0-based, end exclusive). An added hunk has no old rows,
//! a deleted one no new rows (its new rows are empty at the row the deleted
//! lines were before).

use std::ops::Range;
use std::time::Duration;

use similar::{Algorithm, DiffTag, TextDiff};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub old: Range<u32>,
    pub new: Range<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkKind {
    Added,
    Modified,
    Deleted,
}

impl Hunk {
    pub fn kind(&self) -> HunkKind {
        match (self.old.is_empty(), self.new.is_empty()) {
            (true, _) => HunkKind::Added,
            (_, true) => HunkKind::Deleted,
            _ => HunkKind::Modified,
        }
    }

    /// The same change the other way round.
    pub fn inverted(&self) -> Hunk {
        Hunk { old: self.new.clone(), new: self.old.clone() }
    }
}

/// Lines with their newlines; a last line without one gets one, so a file
/// saved with a final newline (as zcode saves) compares equal to one without.
pub fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn normalized(text: &str) -> std::borrow::Cow<'_, str> {
    if text.is_empty() || text.ends_with('\n') {
        text.into()
    } else {
        format!("{text}\n").into()
    }
}

/// The hunks that turn `old` into `new`.
pub fn line_hunks(old: &str, new: &str) -> Vec<Hunk> {
    let (old, new) = (normalized(old), normalized(new));
    if old == new {
        return vec![];
    }
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        // A pathological pair of huge files gives a coarser diff, not a hang.
        .timeout(Duration::from_millis(500))
        .diff_lines(old.as_ref(), new.as_ref());
    let mut hunks: Vec<Hunk> = vec![];
    for op in diff.ops() {
        let (tag, old, new) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            continue;
        }
        let (old, new) = (old.start as u32..old.end as u32, new.start as u32..new.end as u32);
        match hunks.last_mut() {
            // A deletion next to an insertion is one modified hunk.
            Some(last) if last.old.end == old.start && last.new.end == new.start => {
                last.old.end = old.end;
                last.new.end = new.end;
            }
            _ => hunks.push(Hunk { old, new }),
        }
    }
    hunks
}

/// `old` with `hunks` (from `line_hunks(old, new)`) applied: their old rows
/// replaced by their new rows from `new`.
pub fn apply(old: &str, new: &str, hunks: &[&Hunk]) -> String {
    let (old_norm, new_norm) = (normalized(old), normalized(new));
    let (old_lines, new_lines) = (lines(&old_norm), lines(&new_norm));
    let mut hunks: Vec<&&Hunk> = hunks.iter().collect();
    hunks.sort_by_key(|h| h.old.start);
    let mut out = String::with_capacity(old.len().max(new.len()));
    let mut at = 0usize;
    for hunk in hunks {
        let start = (hunk.old.start as usize).min(old_lines.len());
        if start < at {
            continue;
        }
        out.extend(old_lines[at..start].iter().copied());
        out.extend(slice(&new_lines, &hunk.new).iter().copied());
        at = (hunk.old.end as usize).min(old_lines.len());
    }
    out.extend(old_lines[at..].iter().copied());
    out
}

/// The lines of `rows`, clamped to what there is.
pub fn slice<'a>(lines: &'a [&'a str], rows: &Range<u32>) -> &'a [&'a str] {
    let end = (rows.end as usize).min(lines.len());
    &lines[(rows.start as usize).min(end)..end]
}

/// Whether two row ranges of one text touch: they overlap, or one is empty
/// (a deletion's position) and lies within or at an edge of the other.
pub fn touches(a: &Range<u32>, b: &Range<u32>) -> bool {
    if a.is_empty() || b.is_empty() {
        a.start <= b.end && b.start <= a.end
    } else {
        a.start < b.end && b.start < a.end
    }
}

/// How much of a change against `HEAD` is in the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Staging {
    /// None of it: the index still has `HEAD`'s lines.
    Unstaged,
    /// All of it: the index has the editor's lines.
    Staged,
    /// Some of it.
    Partial,
}

/// The staging of `hunk` (`HEAD` → editor), given the index → editor hunks
/// and the `HEAD` and index lines.
pub fn staging(hunk: &Hunk, index_hunks: &[Hunk], head: &[&str], index: &[&str]) -> Staging {
    let overlapping: Vec<&Hunk> = index_hunks.iter().filter(|h| touches(&h.new, &hunk.new)).collect();
    match overlapping[..] {
        [] => Staging::Staged,
        [only] if only.new == hunk.new && slice(head, &hunk.old) == slice(index, &only.old) => Staging::Unstaged,
        _ => Staging::Partial,
    }
}

/// The index's text once `hunk` (`HEAD` → editor) is staged: every
/// index → editor hunk touching it applied.
pub fn stage_hunk(hunk: &Hunk, index: &str, current: &str) -> String {
    let index_hunks = line_hunks(index, current);
    let touching: Vec<&Hunk> = index_hunks.iter().filter(|h| touches(&h.new, &hunk.new)).collect();
    apply(index, current, &touching)
}

/// The index's text once `hunk` (`HEAD` → editor) is unstaged: every
/// index → `HEAD` hunk touching its `HEAD` rows reverted.
pub fn unstage_hunk(hunk: &Hunk, index: &str, head: &str) -> String {
    let back = line_hunks(index, head);
    let touching: Vec<&Hunk> = back.iter().filter(|h| touches(&h.new, &hunk.old)).collect();
    apply(index, head, &touching)
}

/// Discard Change: the edit of `text` that gives `hunk` (`HEAD` → `text`)
/// back `HEAD`'s lines, as the byte range to replace and what replaces it.
/// A hunk at the end of the text ends it as `HEAD` ends, with a final
/// newline or without one.
pub fn discard_edit(head: &str, text: &str, hunk: &Hunk) -> (Range<usize>, String) {
    let text_lines = lines(text);
    let offset = |row: u32| text_lines[..(row as usize).min(text_lines.len())].iter().map(|l| l.len()).sum::<usize>();
    let (mut start, end) = (offset(hunk.new.start), offset(hunk.new.end));
    let mut replacement: String = slice(&lines(head), &hunk.old).concat();
    let after_newline = start == 0 || text[..start].ends_with('\n');
    if !replacement.is_empty() && !after_newline {
        // After a last line without a newline.
        replacement.insert(0, '\n');
    } else if replacement.is_empty() && end == text.len() && start > 0 && after_newline && !head.ends_with('\n') {
        // Lines added after `HEAD`'s last line, which has no newline: the
        // newline before them goes too.
        start -= 1;
    }
    (start..end, replacement)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(old: Range<u32>, new: Range<u32>) -> Hunk {
        Hunk { old, new }
    }

    #[test]
    fn hunks_are_runs_of_changed_lines() {
        let old = "a\nb\nc\nd\ne\n";
        assert_eq!(line_hunks(old, old), vec![]);
        // Modified, added, deleted.
        let new = "a\nB\nc\nd\nnew\n";
        assert_eq!(line_hunks(old, new), vec![h(1..2, 1..2), h(4..5, 4..5)]);
        let new = "a\nb\nx\ny\nc\nd\ne\n";
        let hunks = line_hunks(old, new);
        assert_eq!(hunks, vec![h(2..2, 2..4)]);
        assert_eq!(hunks[0].kind(), HunkKind::Added);
        let new = "a\nd\ne\n";
        let hunks = line_hunks(old, new);
        assert_eq!(hunks, vec![h(1..3, 1..1)]);
        assert_eq!(hunks[0].kind(), HunkKind::Deleted);
        // A final newline alone isn't a change.
        assert_eq!(line_hunks("a\nb", "a\nb\n"), vec![]);
        // From and to nothing.
        assert_eq!(line_hunks("", "a\nb\n"), vec![h(0..0, 0..2)]);
        assert_eq!(line_hunks("a\n", ""), vec![h(0..1, 0..0)]);
    }

    #[test]
    fn applying_some_hunks() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "A\nb\nc\nd\ne\nf\n";
        let hunks = line_hunks(old, new);
        assert_eq!(hunks.len(), 2);
        assert_eq!(apply(old, new, &[&hunks[0]]), "A\nb\nc\nd\ne\n");
        assert_eq!(apply(old, new, &[&hunks[1]]), "a\nb\nc\nd\ne\nf\n");
        assert_eq!(apply(old, new, &[&hunks[1], &hunks[0]]), new);
        assert_eq!(apply(old, new, &[]), old);
        // Reverting a hunk is applying its inverse the other way.
        assert_eq!(apply(new, old, &[&hunks[0].inverted()]), "a\nb\nc\nd\ne\nf\n");
    }

    #[test]
    fn discarding_a_hunk_keeps_heads_final_newline_or_its_absence() {
        let discard = |head: &str, text: &str| -> Vec<String> {
            line_hunks(head, text)
                .iter()
                .map(|hunk| {
                    let (range, replacement) = discard_edit(head, text, hunk);
                    format!("{}{replacement}{}", &text[..range.start], &text[range.end..])
                })
                .collect()
        };
        // In the middle; at the end of a file with and without a final newline.
        assert_eq!(discard("a\nb\nc\n", "a\nB\nc\n"), vec!["a\nb\nc\n"]);
        assert_eq!(discard("a\nb\n", "a\nb\nc\n"), vec!["a\nb\n"]);
        assert_eq!(discard("let a = 1\nlet b = 2", "let a = 1\nlet b = 2\nlet c = 3"), vec!["let a = 1\nlet b = 2"]);
        assert_eq!(discard("a\nb", "a\nb\nc\n"), vec!["a\nb"]);
        assert_eq!(discard("a\nb", "a\nX"), vec!["a\nb"]);
        assert_eq!(discard("a\nb\n", "a\nX"), vec!["a\nb\n"]);
        // Deleted lines come back, after a last line without a newline.
        assert_eq!(discard("a\nb\nc", "a\nb"), vec!["a\nb\nc"]);
        assert_eq!(discard("a\nb\nc\n", "a\n"), vec!["a\nb\nc\n"]);
        assert_eq!(discard("a\nb\nc\n", "b\nc\n"), vec!["a\nb\nc\n"]);
        // Two hunks, each on its own.
        assert_eq!(discard("a\nb\nc", "A\nb\nc\nd"), vec!["a\nb\nc\nd", "A\nb\nc"]);
        // A file new since HEAD empties.
        assert_eq!(discard("", "x\ny\n"), vec![""]);
    }

    #[test]
    fn touching_ranges() {
        assert!(touches(&(1..3), &(2..4)));
        assert!(!touches(&(1..3), &(3..5)));
        // A deletion at row 3 touches the hunk ending or starting there.
        assert!(touches(&(3..3), &(1..3)) && touches(&(3..3), &(3..5)));
        assert!(!touches(&(3..3), &(5..6)));
    }

    #[test]
    fn staging_of_head_hunks() {
        let head = "a\nb\nc\nd\ne\n";
        let current = "A\nb\nc\nd\nE\n";
        // The index has the first change only.
        let index = "A\nb\nc\nd\ne\n";
        let head_hunks = line_hunks(head, current);
        let index_hunks = line_hunks(index, current);
        let (hl, il) = (lines(head), lines(index));
        let states: Vec<Staging> = head_hunks.iter().map(|hunk| staging(hunk, &index_hunks, &hl, &il)).collect();
        assert_eq!(states, vec![Staging::Staged, Staging::Unstaged]);
        // Half of a two-line change staged.
        let current = "A\nB\nc\nd\ne\n";
        let index = "A\nb\nc\nd\ne\n";
        let head_hunks = line_hunks(head, current);
        assert_eq!(head_hunks, vec![h(0..2, 0..2)]);
        let index_hunks = line_hunks(index, current);
        assert_eq!(staging(&head_hunks[0], &index_hunks, &hl, &lines(index)), Staging::Partial);
    }

    #[test]
    fn stage_and_unstage_one_hunk() {
        let head = "a\nb\nc\nd\ne\n";
        let current = "A\nb\nc\nd\nE\nf\n";
        let hunks = line_hunks(head, current);
        assert_eq!(hunks.len(), 2);
        let index = stage_hunk(&hunks[1], head, current);
        assert_eq!(index, "a\nb\nc\nd\nE\nf\n");
        let index = stage_hunk(&hunks[0], &index, current);
        assert_eq!(index, current);
        let index = unstage_hunk(&hunks[1], &index, head);
        assert_eq!(index, "A\nb\nc\nd\ne\n");
        assert_eq!(unstage_hunk(&hunks[0], &index, head), head);
    }
}

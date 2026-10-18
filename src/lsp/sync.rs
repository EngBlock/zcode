//! What a `textDocument/didChange` sends a server that takes incremental
//! changes: the editor's own edits since the text it last sent
//! ([`zcode_editor::EditJournal`]), or the one range where the texts differ.
//!
//! Both run on the main thread, often from a provider called as a key is
//! typed, so neither reads the whole document. The text sent last and the
//! current one are clones of one rope that share every chunk the edits
//! didn't touch, and a shared chunk is the same allocation in both: comparing
//! them costs one address check per chunk plus the bytes of the chunks that
//! differ.

use std::ops::Range;

use serde_json::{Value, json};
use zcode_editor::{BufferEdit, Rope};

use super::offset_to_lsp;

/// The content changes that take a server from `synced` to `current`: one
/// per edit in `edits` (each in the text the ones before it made, as LSP
/// applies them), with edits that run into each other merged. `None` when
/// the edits don't make `current` from `synced`, for whatever reason (the
/// caller then sends [`diff_change`]).
pub fn edit_changes(synced: &Rope, edits: &[BufferEdit], current: &Rope) -> Option<Vec<Value>> {
    let mut text = synced.clone();
    let mut changes = Vec::new();
    for edit in coalesce(edits) {
        let range = edit.range;
        if range.start > range.end || range.end > text.len() || !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end) {
            return None;
        }
        changes.push(json!({
            "range": { "start": offset_to_lsp(&text, range.start), "end": offset_to_lsp(&text, range.end) },
            "text": edit.new_text,
        }));
        text.remove(range.clone());
        text.insert(range.start, &edit.new_text);
    }
    same_text(&text, current).then_some(changes)
}

/// The one content change that takes a server from `synced` to `current`:
/// the range where they differ.
pub fn diff_change(synced: &Rope, current: &Rope) -> Value {
    let (old, new) = changed_range(synced, current);
    json!({
        "range": { "start": offset_to_lsp(synced, old.start), "end": offset_to_lsp(synced, old.end) },
        "text": current.slice(new).to_string(),
    })
}

/// Merges each edit into the one before it when it touches the text that
/// one inserted (typing a word, deleting what was just typed), so a pause's
/// worth of typing is one change. Edits elsewhere (other cursors) stay apart.
fn coalesce(edits: &[BufferEdit]) -> Vec<BufferEdit> {
    let mut merged: Vec<BufferEdit> = Vec::with_capacity(edits.len());
    for edit in edits {
        if let Some(last) = merged.last_mut() {
            // Where the last edit's text sits now.
            let inserted = last.range.start..last.range.start + last.new_text.len();
            if edit.range.start <= inserted.end && edit.range.end >= inserted.start {
                let before = edit.range.start.saturating_sub(inserted.start);
                let after = edit.range.end.saturating_sub(inserted.start).min(last.new_text.len());
                // Text the edit replaces past either end of the last one's
                // was there before it too.
                let start = last.range.start.min(edit.range.start);
                let end = last.range.end + edit.range.end.saturating_sub(inserted.end);
                if let (Some(head), Some(tail)) = (last.new_text.get(..before), last.new_text.get(after..)) {
                    last.new_text = format!("{head}{}{tail}", edit.new_text);
                    last.range = start..end;
                    continue;
                }
            }
        }
        merged.push(edit.clone());
    }
    merged
}

/// Whether two ropes hold the same text.
pub fn same_text(a: &Rope, b: &Rope) -> bool {
    a.len() == b.len() && common_prefix(a, b) == a.len()
}

/// What changed between `old` and `new`: the byte range of `old` that was
/// replaced and the byte range of `new` that replaced it (one range around
/// every edit: what's the same at both ends is left out), on character
/// boundaries. Sent as a ranged `didChange`, which servers keeping a copy of
/// the text (vtsls's tsserver) apply more reliably than the whole text again.
pub fn changed_range(old: &Rope, new: &Rope) -> (Range<usize>, Range<usize>) {
    let mut prefix = common_prefix(old, new);
    while prefix > 0 && !(old.is_char_boundary(prefix) && new.is_char_boundary(prefix)) {
        prefix -= 1;
    }
    let most = old.len().min(new.len()) - prefix;
    let mut suffix = common_suffix(old, new, most);
    while suffix > 0 && !(old.is_char_boundary(old.len() - suffix) && new.is_char_boundary(new.len() - suffix)) {
        suffix -= 1;
    }
    (prefix..old.len() - suffix, prefix..new.len() - suffix)
}

/// The length in bytes of the text `a` and `b` start with.
fn common_prefix(a: &Rope, b: &Rope) -> usize {
    let (mut a_chunks, mut b_chunks) = (a.chunks(), b.chunks());
    let (mut x, mut y): (&[u8], &[u8]) = (&[], &[]);
    let mut common = 0;
    loop {
        if x.is_empty() {
            let Some(chunk) = a_chunks.next() else { return common };
            x = chunk.as_bytes();
        }
        if y.is_empty() {
            let Some(chunk) = b_chunks.next() else { return common };
            y = chunk.as_bytes();
        }
        let n = matching(x, y, |x, y| x.iter().zip(y).take_while(|(p, q)| p == q).count());
        common += n;
        if n < x.len().min(y.len()) {
            return common;
        }
        (x, y) = (&x[n..], &y[n..]);
    }
}

/// The length in bytes of the text `a` and `b` end with, at most `most`.
fn common_suffix(a: &Rope, b: &Rope, most: usize) -> usize {
    let (mut a_chunks, mut b_chunks) = (a.chunks_at(a.len()).0, b.chunks_at(b.len()).0);
    let (mut x, mut y): (&[u8], &[u8]) = (&[], &[]);
    let mut common = 0;
    loop {
        if common >= most {
            return most;
        }
        if x.is_empty() {
            let Some(chunk) = a_chunks.prev() else { return common };
            x = chunk.as_bytes();
        }
        if y.is_empty() {
            let Some(chunk) = b_chunks.prev() else { return common };
            y = chunk.as_bytes();
        }
        let len = x.len().min(y.len());
        let (x_end, y_end) = (&x[x.len() - len..], &y[y.len() - len..]);
        let n = matching(x_end, y_end, |x, y| x.iter().rev().zip(y.iter().rev()).take_while(|(p, q)| p == q).count());
        common += n;
        if n < len {
            return common.min(most);
        }
        (x, y) = (&x[..x.len() - n], &y[..y.len() - n]);
    }
}

/// How many bytes of the overlap of `x` and `y` match, by `count` when
/// they differ: an equal overlap is one comparison (none for a shared chunk).
fn matching(x: &[u8], y: &[u8], count: impl Fn(&[u8], &[u8]) -> usize) -> usize {
    let len = x.len().min(y.len());
    if std::ptr::eq(x, y) {
        return len;
    }
    // The overlap is the start of both for a prefix, and `count` is only
    // given equal-length slices for a suffix.
    let (x, y) = (&x[..len], &y[..len]);
    if x == y { len } else { count(x, y) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope(text: &str) -> Rope {
        Rope::from(text)
    }

    /// Applies LSP content changes to `text`, as a server does.
    fn apply(text: &str, changes: &[Value]) -> String {
        let mut text = rope(text);
        for change in changes {
            let range: lsp_types::Range = serde_json::from_value(change["range"].clone()).unwrap();
            let range = super::super::lsp_range_to_offsets(&text, range);
            text.remove(range.clone());
            text.insert(range.start, change["text"].as_str().unwrap());
        }
        text.to_string()
    }

    #[test]
    fn changes_are_one_range() {
        let range = |a: &str, b: &str| changed_range(&rope(a), &rope(b));
        assert_eq!(range("let a = 1\n", "let ab = 1\n"), (5..5, 5..6));
        assert_eq!(range("abc", "abc"), (3..3, 3..3));
        assert_eq!(range("aXc\nY", "aZc\nW"), (1..5, 1..5));
        assert_eq!(range("", "new"), (0..0, 0..3));
        // Never inside a character.
        assert_eq!(range("é", "è"), (0..2, 0..2));
        assert_eq!(range("aaa", "aa"), (2..3, 2..2));
    }

    #[test]
    fn large_texts_compare_by_chunk() {
        let base: String = (0..20_000).map(|i| format!("line {i} é\n")).collect();
        let old = rope(&base);
        // An edited clone shares every other chunk with the original.
        let mut new = old.clone();
        new.insert(100_000, "inserted");
        assert_eq!(changed_range(&old, &new), (100_000..100_000, 100_000..100_008));
        new.remove(5..9);
        assert_eq!(changed_range(&old, &new), (5..100_000, 5..100_004));
        // Built separately, nothing is shared.
        let mut separate = base.clone();
        separate.replace_range(base.len() - 3..base.len() - 1, "è");
        let last = base.len() - 3..base.len() - 1;
        assert_eq!(changed_range(&old, &rope(&separate)), (last.clone(), last));
        assert!(same_text(&old, &rope(&base)));
        assert!(same_text(&old, &old.clone()));
        assert!(!same_text(&old, &new));
        // The suffix never runs into the prefix.
        assert_eq!(changed_range(&rope(&base), &rope(&format!("{base}{base}"))), (base.len()..base.len(), base.len()..2 * base.len()));
    }

    #[test]
    fn edits_become_changes_in_order() {
        let synced = "let a = 1\nlet b = 2\n";
        let edit = |range: Range<usize>, text: &str| BufferEdit { range, new_text: text.into() };
        // Typing a word and fixing it is one change; the other cursor's are others.
        let edits = [edit(14..14, "c"), edit(4..4, "x"), edit(5..5, "y"), edit(6..6, "z"), edit(6..7, ""), edit(17..18, "")];
        let edits = &edits[..];
        let mut expected = synced.to_string();
        for e in edits {
            expected.replace_range(e.range.clone(), &e.new_text);
        }
        let current = rope(&expected);
        let changes = edit_changes(&rope(synced), edits, &current).unwrap();
        assert_eq!(changes.len(), 3, "{changes:?}");
        assert_eq!(apply(synced, &changes), expected);
        // Edits that don't make the current text aren't sent.
        assert_eq!(edit_changes(&rope(synced), &edits[1..], &current), None);
        assert_eq!(edit_changes(&rope(synced), &[edit(40..41, "")], &current), None);
        assert_eq!(edit_changes(&rope("é"), &[edit(1..2, "")], &rope("é")), None);
    }

    #[test]
    fn merged_edits_make_the_same_text() {
        // Pseudo-random edits around a few spots, merged or not, give the
        // text applying each in turn gives.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        for _ in 0..300 {
            let start = "fn main() {\n    println!(\"hé\");\n}\n";
            let mut text = start.to_string();
            let mut edits = vec![];
            for _ in 0..1 + next(8) {
                let boundaries: Vec<usize> = (0..=text.len()).filter(|&i| text.is_char_boundary(i)).collect();
                let at = boundaries[next(boundaries.len())];
                let end = boundaries.iter().copied().filter(|&i| i >= at).nth(next(3)).unwrap_or(at);
                let insert = ["", "a", "é", "\n", "xy"][next(5)];
                edits.push(BufferEdit { range: at..end, new_text: insert.into() });
                text.replace_range(at..end, insert);
            }
            let merged = coalesce(&edits);
            let mut replayed = start.to_string();
            for e in &merged {
                replayed.replace_range(e.range.clone(), &e.new_text);
            }
            assert_eq!(replayed, text, "{edits:?} → {merged:?}");
            let changes = edit_changes(&rope(start), &edits, &rope(&text)).unwrap();
            assert_eq!(apply(start, &changes), text);
        }
    }
}

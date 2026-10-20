use std::ops::Range;

#[derive(Clone, Copy)]
enum CharacterKind {
    Word,
    Whitespace,
    Newline,
    Other,
}

impl From<char> for CharacterKind {
    fn from(character: char) -> Self {
        if character == '_'
            || character.is_ascii_alphanumeric()
            || matches!(character, '\u{00C0}'..='\u{024F}' | '\u{0400}'..='\u{04FF}' | '\u{1E00}'..='\u{1EFF}' | '\u{0300}'..='\u{036F}')
        {
            Self::Word
        } else if matches!(character, '\n' | '\r') {
            Self::Newline
        } else if character.is_whitespace() {
            Self::Whitespace
        } else {
            Self::Other
        }
    }
}

pub(crate) fn word_range_from_chars(
    offset: usize,
    character: char,
    previous: impl Iterator<Item = char>,
    following: impl Iterator<Item = char>,
) -> Range<usize> {
    let kind = CharacterKind::from(character);
    let connects = |next| {
        matches!(
            (kind, CharacterKind::from(next)),
            (CharacterKind::Word, CharacterKind::Word)
                | (CharacterKind::Whitespace, CharacterKind::Whitespace)
        )
    };
    let start = previous
        .take(128)
        .take_while(|character| connects(*character))
        .fold(offset, |offset, character| offset - character.len_utf8());
    let end = following
        .take(128)
        .take_while(|character| connects(*character))
        .fold(offset + character.len_utf8(), |offset, character| {
            offset + character.len_utf8()
        });
    start..end
}


/// How far either side of an offset grapheme clusters are segmented. A
/// cluster longer than this (stacked combining marks) is stepped through in
/// pieces, which keeps a step's cost independent of the line's length.
const GRAPHEME_WINDOW: usize = 128;

/// The text around `offset` that its grapheme clusters are found in, and
/// where that text starts.
fn grapheme_window(text: &ropey::Rope, offset: usize) -> (usize, usize, String) {
    use crate::RopeExt as _;
    use sum_tree::Bias;
    let offset = text.clip_offset(offset, Bias::Left);
    let start = text.clip_offset(offset.saturating_sub(GRAPHEME_WINDOW), Bias::Left);
    let end = text.clip_offset((offset + GRAPHEME_WINDOW).min(text.len()), Bias::Right);
    (offset, start, text.slice(start..end).to_string())
}

/// The start of the extended grapheme cluster before `offset` (the one
/// `offset` is inside, if it splits one): where ← and Backspace go, so an
/// emoji with a skin tone, a family emoji's ZWJ sequence or a letter with a
/// combining accent is one step.
pub(crate) fn previous_grapheme_boundary(text: &ropey::Rope, offset: usize) -> usize {
    let (offset, start, window) = grapheme_window(text, offset);
    let mut cursor = unicode_segmentation::GraphemeCursor::new(offset - start, window.len(), true);
    match cursor.prev_boundary(&window, 0) {
        Ok(Some(boundary)) => start + boundary,
        _ => offset.saturating_sub(1),
    }
}

/// The end of the extended grapheme cluster after `offset`: where → and
/// forward delete go.
pub(crate) fn next_grapheme_boundary(text: &ropey::Rope, offset: usize) -> usize {
    let (offset, start, window) = grapheme_window(text, offset);
    let mut cursor = unicode_segmentation::GraphemeCursor::new(offset - start, window.len(), true);
    match cursor.next_boundary(&window, 0) {
        Ok(Some(boundary)) => start + boundary,
        _ => (offset + 1).min(text.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grapheme_steps() {
        // a, 👍🏽 (thumbs up + skin tone), b; e + combining acute; the family
        // emoji is four people joined by ZWJs.
        let text = ropey::Rope::from("a👍🏽b e\u{301}x 👨‍👩‍👧‍👦!\r\nz");
        let s = text.to_string();
        let thumbs = s.find('👍').unwrap();
        let after_thumbs = s.find('b').unwrap();
        assert_eq!(next_grapheme_boundary(&text, thumbs), after_thumbs);
        assert_eq!(previous_grapheme_boundary(&text, after_thumbs), thumbs);
        // An offset inside a cluster goes to its edges.
        assert_eq!(next_grapheme_boundary(&text, thumbs + 4), after_thumbs);
        assert_eq!(previous_grapheme_boundary(&text, thumbs + 4), thumbs);
        let e = s.find('e').unwrap();
        assert_eq!(next_grapheme_boundary(&text, e), s.find('x').unwrap());
        assert_eq!(previous_grapheme_boundary(&text, s.find('x').unwrap()), e);
        let family = s.find('👨').unwrap();
        assert_eq!(next_grapheme_boundary(&text, family), s.find('!').unwrap());
        assert_eq!(previous_grapheme_boundary(&text, s.find('!').unwrap()), family);
        // CRLF is one cluster; the ends of the text stay put.
        let cr = s.find('\r').unwrap();
        assert_eq!(next_grapheme_boundary(&text, cr), cr + 2);
        assert_eq!(previous_grapheme_boundary(&text, cr + 2), cr);
        assert_eq!(previous_grapheme_boundary(&text, 0), 0);
        assert_eq!(next_grapheme_boundary(&text, s.len()), s.len());
    }

    #[test]
    fn a_cluster_longer_than_the_window_still_moves() {
        let text = ropey::Rope::from(format!("a{}b", "\u{301}".repeat(200)));
        let next = next_grapheme_boundary(&text, 0);
        assert!(next > 0 && next < text.len() - 1);
        assert!(previous_grapheme_boundary(&text, text.len()) < text.len());
    }
}

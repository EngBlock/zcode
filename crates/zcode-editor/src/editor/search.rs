use crate::InputModeKind;
use aho_corasick::AhoCorasick;
use gpui::{Context, Window};
use ropey::Rope;
use std::{ops::Range, rc::Rc};

use super::{
    InputBaseState, Replace, RopeExt as _, Search, movement::MoveDirection, state::ScrollPadding,
};

/// Stateful, presentation-independent search engine used by text inputs.
#[derive(Debug, Clone)]
pub struct SearchMatcher {
    text: Rope,
    pub query: Option<AhoCorasick>,
    matched_ranges: Rc<Vec<Range<usize>>>,
    current_match_ix: usize,
    /// Set by a replacement: the offset just past the inserted text, where
    /// the next scan picks the current match.
    replacing: Option<usize>,
}

/// One search over an input: the query, how the built-in panel shows it, and
/// its matches. Read it through [`InputBaseState::search_session`]; it is
/// written only through the input state's search methods, and it grows, so
/// build it with `Default` and do not destructure it exhaustively.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SearchSession {
    /// The built-in search panel is showing.
    pub open: bool,
    pub replace_mode: bool,
    pub case_insensitive: bool,
    pub query: String,
    pub replacement: String,
    pub anchor_offset: Option<usize>,
    pub matcher: SearchMatcher,
    /// A search is in progress and its matches are highlighted: the panel is
    /// open, or a query was set without it and not closed since.
    active: bool,
}

impl Default for SearchSession {
    fn default() -> Self {
        Self {
            open: false,
            active: false,
            replace_mode: false,
            case_insensitive: true,
            query: String::new(),
            replacement: String::new(),
            anchor_offset: None,
            matcher: SearchMatcher::new(),
        }
    }
}

impl SearchSession {
    pub(crate) fn open(&mut self, replace_mode: bool, replaceable: bool) {
        self.open = true;
        self.active = true;
        self.replace_mode = replace_mode && replaceable;
    }

    /// Start a search without the built-in panel. A custom search UI drives
    /// the session through [`InputBaseState::set_search_query`], and the
    /// editor highlights the matches the same way it does for the panel.
    pub(crate) fn activate(&mut self) {
        self.active = true;
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
        self.active = false;
    }

    /// Whether a search is in progress: the built-in panel is open, or a
    /// query was set without it and [`InputBaseState::close_search`] has not
    /// run since. Matches are highlighted while this holds.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Returns whether the query or its case sensitivity changed.
    pub(crate) fn update_query(&mut self, query: impl Into<String>, case_insensitive: bool) -> bool {
        let query = query.into();
        if self.query == query && self.case_insensitive == case_insensitive {
            return false;
        }

        self.query = query;
        self.case_insensitive = case_insensitive;
        self.matcher.update_query(&self.query, case_insensitive);
        true
    }
}

impl<M: InputModeKind> InputBaseState<M> {
    /// Open the search session, or re-invoke it if it is already open.
    ///
    /// This is not idempotent: every call advances
    /// [`InputBaseState::search_activation_revision`], and the presentation
    /// layer answers that by re-focusing the search field and selecting its
    /// contents, the same as pressing the shortcut a second time. Call it from
    /// an action or another user gesture, never from a render pass or an
    /// observer that runs every frame — that would re-select the field under
    /// the user on every frame and make it impossible to type.
    pub fn open_search(&mut self, replace_mode: bool, cx: &mut Context<Self>) {
        if !self.searchable {
            return;
        }
        self.search_activation_revision = self.search_activation_revision.wrapping_add(1);
        self.search_session
            .open(replace_mode, self.is_replaceable());
        let selected = self.selected_text().to_string();
        let query = if selected.is_empty() {
            self.search_session.query.clone()
        } else {
            selected
        };
        let query_changed = query != self.search_session.query;
        // A retained query resumes its previous occurrence. Only a new query
        // is anchored to the current viewport.
        self.search_session.anchor_offset = if query_changed {
            self.last_layout
                .as_ref()
                .map(|layout| layout.visible_range_offset.start)
        } else {
            None
        };
        let case_insensitive = self.search_session.case_insensitive;
        self.search_session.update_query(query, case_insensitive);
        self.search_session.matcher.update(&self.text);
        if query_changed && let Some(anchor) = self.search_session.anchor_offset {
            self.search_session.matcher.update_cursor_by_offset(anchor);
        }
        cx.notify();
    }

    pub fn search_session(&self) -> &SearchSession {
        &self.search_session
    }

    /// A counter that advances every time [`InputBaseState::open_search`] runs,
    /// including while the session is already open.
    ///
    /// Re-invoking search leaves the session itself identical, so a presentation
    /// layer that decides what to rebuild by comparing session state cannot see
    /// the second request. Fold this into that comparison to notice it.
    pub fn search_activation_revision(&self) -> u64 {
        self.search_activation_revision
    }

    #[doc(hidden)]
    pub fn set_search_replace_mode(&mut self, replace_mode: bool, cx: &mut Context<Self>) {
        self.search_session.replace_mode = replace_mode && self.is_replaceable();
        cx.notify();
    }

    /// Returns true if the search panel can replace the matches.
    ///
    /// This is false when the input is not `replaceable`, or when it is
    /// `disabled` or `readonly`.
    pub fn is_replaceable(&self) -> bool {
        self.replaceable && self.is_editable()
    }

    /// Set the search query and highlight its matches.
    ///
    /// This is the entry point for a custom search UI: it needs neither
    /// `searchable` nor the built-in panel. Navigate the matches with
    /// [`InputBaseState::next_search_match`] and
    /// [`InputBaseState::previous_search_match`], read the count and the
    /// current index from [`InputBaseState::search_session`], and end the
    /// search with [`InputBaseState::close_search`].
    ///
    /// A new query selects its first match at or after the selection (the
    /// first one in the document when none follows) and scrolls to it, as
    /// typing in Xcode's or VS Code's find field does, so the caret is at the
    /// match when the search ends. Setting the same query again moves nothing.
    pub fn set_search_query(
        &mut self,
        query: impl Into<String>,
        case_insensitive: bool,
        cx: &mut Context<Self>,
    ) {
        self.search_session.activate();
        let changed = self.search_session.update_query(query, case_insensitive);
        self.search_session.matcher.update(&self.text);
        if changed {
            let anchor = self.selected_range().start;
            let matcher = &mut self.search_session.matcher;
            matcher.set_current_at_or_after(anchor);
            if let Some(range) = matcher.current_range() {
                self.select_search_match(range, cx);
            }
        }
        cx.notify();
    }

    /// End the search: hide the built-in panel and the match highlights. The
    /// query is kept so the next [`InputBaseState::open_search`] resumes it.
    pub fn close_search(&mut self, cx: &mut Context<Self>) {
        self.search_session.close();
        cx.notify();
    }

    /// Move to the next match and select it, so ending the search leaves the
    /// caret there (Xcode, VS Code and Zed do the same).
    pub fn next_search_match(&mut self, cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.sync_search_matcher();
        let range = self.search_session.matcher.next()?;
        self.select_search_match(range.clone(), cx);
        Some(range)
    }

    /// Move to the previous match and select it.
    pub fn previous_search_match(&mut self, cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.sync_search_matcher();
        let range = self.search_session.matcher.next_back()?;
        self.select_search_match(range.clone(), cx);
        Some(range)
    }

    /// Select a match and bring it into view.
    fn select_search_match(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.set_selected_range(range.clone(), cx);
        // Match order does not describe viewport direction after a manual
        // scroll. Always allow search navigation to reveal the active match.
        self.scroll_to_with_padding(range.start, None, ScrollPadding::SurroundingLines, cx);
    }

    /// Replace the current match and move on to the next one, the first that
    /// starts after the inserted text (wrapping to the top), so a replacement
    /// that contains the query is never matched again. The next match is
    /// selected. Returns whether there was a match to replace.
    pub fn replace_current_search_match(
        &mut self,
        replacement: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.is_replaceable() {
            return false;
        }
        self.sync_search_matcher();
        let Some(range) = self.search_session.matcher.current_range() else {
            return false;
        };
        // The edit rescans the matches; the current one becomes the first
        // past the inserted text.
        self.search_session
            .matcher
            .begin_replacement(range.start + replacement.len());
        let range_utf16 = self.range_to_utf16(&range);
        self.replace_text_in_range_silent(Some(range_utf16), replacement, window, cx);
        self.sync_search_matcher();
        if let Some(next) = self.search_session.matcher.current_range() {
            self.select_search_match(next, cx);
        }
        true
    }

    /// Replace every match. Returns how many were replaced.
    pub fn replace_all_search_matches(
        &mut self,
        replacement: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        if !self.is_replaceable() {
            return 0;
        }
        self.sync_search_matcher();
        let ranges = self.search_session.matcher.matched_ranges();
        if ranges.is_empty() {
            return 0;
        }
        let mut text = self.text.clone();
        for range in ranges.iter().rev() {
            text.replace(range.clone(), replacement);
        }
        self.search_session.matcher.begin_replacement(0);
        let count = ranges.len();
        self.replace_text_in_range_silent(Some(0..self.text.len()), &text.to_string(), window, cx);
        self.scroll_to(0, Some(MoveDirection::Down), cx);
        count
    }

    /// Keep the matches in step with an edit. A closed search skips the scan:
    /// it copies and searches the whole document, and nothing reads the
    /// matches until the search is resumed or navigated, which sync first.
    pub(super) fn update_search(&mut self, _cx: &mut gpui::App) {
        if !self.search_session.is_active() {
            return;
        }
        self.sync_search_matcher();
    }

    /// Recompute the matches if the text changed since the last scan.
    fn sync_search_matcher(&mut self) {
        self.search_session.matcher.update(&self.text);
    }

    /// An input that is not `searchable` leaves the shortcut to its
    /// ancestors, so a custom search UI can take it.
    pub(super) fn on_action_search(&mut self, _: &Search, _: &mut Window, cx: &mut Context<Self>) {
        if !self.searchable {
            cx.propagate();
            return;
        }
        self.open_search(false, cx);
    }

    pub(super) fn on_action_replace(
        &mut self,
        _: &Replace,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.searchable {
            cx.propagate();
            return;
        }
        self.open_search(true, cx);
    }
}

impl Default for SearchMatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchMatcher {
    pub fn new() -> Self {
        Self {
            text: "".into(),
            query: None,
            matched_ranges: Rc::new(Vec::new()),
            current_match_ix: 0,
            replacing: None,
        }
    }

    /// Update the source text and recompute matches.
    pub fn update(&mut self, text: &Rope) {
        if self.text.eq(text) {
            self.replacing = None;
            return;
        }
        self.text = text.clone();
        self.update_matches();
    }

    pub fn update_query(&mut self, query: &str, case_insensitive: bool) {
        self.query = (!query.is_empty()).then(|| {
            AhoCorasick::builder()
                .ascii_case_insensitive(case_insensitive)
                .build([query])
                .expect("failed to build input search query")
        });
        self.update_matches();
    }

    pub fn matched_ranges(&self) -> Rc<Vec<Range<usize>>> {
        self.matched_ranges.clone()
    }

    pub fn current_match_index(&self) -> usize {
        self.current_match_ix
    }

    /// The index of the current match into [`SearchMatcher::matched_ranges`],
    /// `None` while there is no match.
    pub fn current(&self) -> Option<usize> {
        (!self.is_empty()).then_some(self.current_match_ix)
    }

    pub fn len(&self) -> usize {
        self.matched_ranges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.matched_ranges.is_empty()
    }

    /// `2/5`: the current match and the total, `0/0` without matches.
    pub fn label(&self) -> String {
        match self.current() {
            Some(ix) => format!("{}/{}", ix + 1, self.len()),
            None => "0/0".into(),
        }
    }

    /// The current match, `None` while there is no match.
    pub fn current_range(&self) -> Option<Range<usize>> {
        self.matched_ranges.get(self.current_match_ix).cloned()
    }

    /// Make the first match starting at or after `offset` the current one,
    /// the first match when none does.
    fn set_current_at_or_after(&mut self, offset: usize) {
        self.current_match_ix = self
            .matched_ranges
            .iter()
            .position(|range| range.start >= offset)
            .unwrap_or(0);
    }

    fn has_next_without_wrap(&self) -> bool {
        self.current_match_ix < self.matched_ranges.len().saturating_sub(1)
    }

    pub fn update_cursor_by_offset(&mut self, offset: usize) {
        for (ix, range) in self.matched_ranges.iter().enumerate() {
            self.current_match_ix = ix;
            if range.contains(&offset) || range.end >= offset {
                return;
            }
        }
    }

    /// A replacement is about to change the text: the next scan makes the
    /// first match starting at or after `resume_at` (the end of the inserted
    /// text, in the new text) the current one, wrapping to the first.
    fn begin_replacement(&mut self, resume_at: usize) {
        self.replacing = Some(resume_at);
    }

    #[cfg(test)]
    fn set_current_match_index(&mut self, index: usize) {
        self.current_match_ix = index.min(self.matched_ranges.len().saturating_sub(1));
    }

    fn next_index(&self) -> Option<usize> {
        if self.is_empty() {
            None
        } else if self.has_next_without_wrap() {
            Some(self.current_match_ix + 1)
        } else {
            Some(0)
        }
    }

    fn update_matches(&mut self) {
        let mut ranges = Vec::new();
        if let Some(query) = &self.query {
            let text = self.text.to_string();
            ranges.extend(
                query
                    .stream_find_iter(text.as_bytes())
                    .map(|result| result.expect("input search match").range()),
            );
        }
        self.matched_ranges = Rc::new(ranges);
        match self.replacing.take() {
            Some(resume_at) => self.set_current_at_or_after(resume_at),
            None => self.current_match_ix = 0,
        }
    }
}

impl Iterator for SearchMatcher {
    type Item = Range<usize>;

    fn next(&mut self) -> Option<Self::Item> {
        let ix = self.next_index()?;
        self.current_match_ix = ix;
        self.matched_ranges.get(ix).cloned()
    }
}

impl DoubleEndedIterator for SearchMatcher {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.is_empty() {
            return None;
        }
        if self.current_match_ix == 0 {
            self.current_match_ix = self.len();
        }
        self.current_match_ix -= 1;
        self.matched_ranges.get(self.current_match_ix).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_navigates() {
        let mut matcher = SearchMatcher::new();
        matcher.update(&Rope::from("foo FOO foo"));
        matcher.update_query("foo", true);
        assert_eq!(&*matcher.matched_ranges(), &[0..3, 4..7, 8..11]);
        assert_eq!(matcher.next(), Some(4..7));
        assert_eq!(matcher.next_back(), Some(0..3));
    }

    #[test]
    fn next_wraps_to_start() {
        let mut matcher = SearchMatcher::new();
        matcher.update(&Rope::from(".....aaaaa.....aaaaa.....aaaaa"));
        matcher.update_query("aaaaa", false);
        matcher.set_current_match_index(2);
        assert_eq!(matcher.next(), Some(5..10));
    }

    #[test]
    fn a_query_set_without_the_panel_keeps_the_session_active_until_closed() {
        let mut session = SearchSession::default();
        assert!(!session.is_active());

        session.open(false, true);
        assert!(session.is_active());
        session.close();
        assert!(!session.is_active());

        // A custom search UI never opens the panel; setting a query is what
        // turns the match highlights on, and closing turns them off again.
        session.activate();
        assert!(session.is_active());
        assert!(!session.open);
        session.close();
        assert!(!session.is_active());
    }

    #[test]
    fn identical_query_keeps_the_current_match() {
        let mut session = SearchSession::default();
        session.update_query("foo", true);
        session.matcher.update(&Rope::from("foo bar foo baz foo"));
        session.matcher.update_cursor_by_offset(12);
        assert_eq!(session.matcher.current_match_index(), 2);

        // Reopening Find and the styled search panel's initial query echo both
        // update the session with the same query. Neither should reset the
        // previously active occurrence.
        session.update_query("foo", true);

        assert_eq!(session.matcher.current_match_index(), 2);
        assert_eq!(session.matcher.label(), "3/3");
    }

    #[test]
    fn a_replacement_moves_on_to_the_first_match_after_the_inserted_text() {
        let mut matcher = SearchMatcher::new();
        matcher.update(&Rope::from("foo foo foo"));
        matcher.update_query("foo", true);
        assert_eq!(matcher.label(), "1/3");

        // `foo` → `bar`: the match that followed becomes the first.
        matcher.begin_replacement(3);
        matcher.update(&Rope::from("bar foo foo"));
        assert_eq!(matcher.current_range(), Some(4..7));
        assert_eq!(matcher.label(), "1/2");

        // The last one replaced wraps to the top.
        matcher.set_current_match_index(1);
        matcher.begin_replacement(11);
        matcher.update(&Rope::from("bar foo bar"));
        assert_eq!(matcher.current_range(), Some(4..7));
        assert_eq!(matcher.label(), "1/1");
    }

    /// `foo` → `foobar` leaves a match inside the inserted text; Replace must
    /// not land on it, or the next Replace gives `foobarbar`.
    #[test]
    fn a_replacement_containing_the_query_is_not_matched_again() {
        let mut matcher = SearchMatcher::new();
        matcher.update(&Rope::from("foo one\nfoo two\nfoo three\n"));
        matcher.update_query("foo", true);
        matcher.begin_replacement(6);
        matcher.update(&Rope::from("foobar one\nfoo two\nfoo three\n"));
        assert_eq!(matcher.current_range(), Some(11..14));
        assert_eq!(matcher.label(), "2/3");

        // The last match replaced wraps to the first, a replacement itself.
        matcher.set_current_match_index(2);
        matcher.begin_replacement(25);
        matcher.update(&Rope::from("foobar one\nfoo two\nfoobar three\n"));
        assert_eq!(matcher.current_range(), Some(0..3));
    }
}

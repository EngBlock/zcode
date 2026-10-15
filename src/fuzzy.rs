//! Fuzzy matching for Open Quickly, Quick Actions and symbol pickers.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

#[derive(Clone, Debug)]
pub struct FuzzyMatch {
    pub index: usize,
    pub score: u32,
    /// Matched character positions, for highlighting.
    pub positions: Vec<usize>,
}

/// Scores `candidates` against `query`, best first. An empty query keeps the
/// original order.
pub fn fuzzy_match<S: AsRef<str>>(
    query: &str,
    candidates: &[S],
    paths: bool,
    limit: usize,
) -> Vec<FuzzyMatch> {
    if query.trim().is_empty() {
        return (0..candidates.len().min(limit))
            .map(|index| FuzzyMatch {
                index,
                score: 0,
                positions: vec![],
            })
            .collect();
    }
    let mut config = Config::DEFAULT;
    if paths {
        config.set_match_paths();
    }
    let mut matcher = Matcher::new(config);
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();
    let mut matches: Vec<FuzzyMatch> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            let haystack = Utf32Str::new(candidate.as_ref(), &mut buf);
            pattern.score(haystack, &mut matcher).map(|score| FuzzyMatch {
                index,
                score,
                positions: vec![],
            })
        })
        .collect();
    matches.sort_by(|a, b| {
        b.score.cmp(&a.score).then_with(|| {
            candidates[a.index]
                .as_ref()
                .len()
                .cmp(&candidates[b.index].as_ref().len())
        })
    });
    matches.truncate(limit);
    let mut indices = Vec::new();
    for m in &mut matches {
        indices.clear();
        let haystack = Utf32Str::new(candidates[m.index].as_ref(), &mut buf);
        pattern.indices(haystack, &mut matcher, &mut indices);
        indices.sort_unstable();
        indices.dedup();
        m.positions = indices.iter().map(|&i| i as usize).collect();
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_file_names() {
        let files = ["Sources/App/ContentView.swift", "Sources/App/Counter.swift", "README.md"];
        let matches = fuzzy_match("cntv", &files, true, 10);
        assert_eq!(matches[0].index, 0);
        assert!(!matches[0].positions.is_empty());
        assert_eq!(fuzzy_match("", &files, true, 2).len(), 2);
    }
}

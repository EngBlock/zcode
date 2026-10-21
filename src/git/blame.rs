//! `git blame --porcelain`: which commit last changed each line, and how
//! long ago, for inline blame and the Authors view.

use std::collections::HashMap;

/// The all-zero id git gives lines that aren't committed.
const UNCOMMITTED: &str = "0000000000000000000000000000000000000000";

#[derive(Clone, Debug, PartialEq)]
pub struct Commit {
    pub sha: String,
    pub author: String,
    pub email: String,
    /// Seconds since 1970.
    pub time: i64,
    pub summary: String,
    pub uncommitted: bool,
}

impl Commit {
    pub fn short_sha(&self) -> &str {
        &self.sha[..self.sha.len().min(8)]
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Blame {
    /// Each line's commit, as an index into `commits`.
    pub lines: Vec<u32>,
    pub commits: Vec<Commit>,
}

impl Blame {
    /// The commit of 0-based `row`.
    pub fn commit(&self, row: u32) -> Option<&Commit> {
        self.commits.get(*self.lines.get(row as usize)? as usize)
    }

    /// The blame of a file with no committed version (untracked, or only
    /// added): each of its `lines` not committed yet, as git blames edits.
    pub fn uncommitted(lines: usize) -> Self {
        let commit = Commit {
            sha: UNCOMMITTED.into(),
            author: "Not Committed Yet".into(),
            email: String::new(),
            time: now(),
            summary: String::new(),
            uncommitted: true,
        };
        Self { lines: vec![0; lines], commits: vec![commit] }
    }
}

pub fn parse_porcelain(out: &str) -> Blame {
    let mut blame = Blame::default();
    let mut by_sha: HashMap<String, u32> = HashMap::new();
    let mut current: Option<u32> = None;
    for line in out.lines() {
        if line.starts_with('\t') {
            if let Some(commit) = current {
                blame.lines.push(commit);
            }
            continue;
        }
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        // "<sha> <orig line> <final line> [<lines>]" starts every line's entry.
        if key.len() == 40 && key.bytes().all(|b| b.is_ascii_hexdigit()) {
            let ix = *by_sha.entry(key.to_string()).or_insert_with(|| {
                blame.commits.push(Commit {
                    sha: key.to_string(),
                    author: String::new(),
                    email: String::new(),
                    time: 0,
                    summary: String::new(),
                    uncommitted: key == UNCOMMITTED,
                });
                (blame.commits.len() - 1) as u32
            });
            current = Some(ix);
            continue;
        }
        let Some(commit) = current.and_then(|ix| blame.commits.get_mut(ix as usize)) else { continue };
        match key {
            // Newer gits say "External file (--contents)".
            "author" if commit.uncommitted => commit.author = "Not Committed Yet".into(),
            "author" => commit.author = value.to_string(),
            "author-mail" => commit.email = value.trim_matches(['<', '>']).to_string(),
            "author-time" => commit.time = value.parse().unwrap_or(0),
            "summary" => commit.summary = value.to_string(),
            _ => {}
        }
    }
    blame
}

/// "just now", "5 minutes ago", "3 days ago", "2 years ago".
pub fn relative_time(time: i64, now: i64) -> String {
    let seconds = (now - time).max(0);
    let (n, unit) = match seconds {
        s if s < 60 => return "just now".into(),
        s if s < 3600 => (s / 60, "minute"),
        s if s < 86_400 => (s / 3600, "hour"),
        s if s < 7 * 86_400 => (s / 86_400, "day"),
        s if s < 30 * 86_400 => (s / (7 * 86_400), "week"),
        s if s < 365 * 86_400 => (s / (30 * 86_400), "month"),
        s => (s / (365 * 86_400), "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `2026-10-17 14:03 UTC`.
pub fn absolute_time(time: i64) -> String {
    let date = crate::plist::format_date(time as f64 - 978_307_200.);
    format!("{} UTC", date.get(..16).unwrap_or(&date).replace('T', " "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_porcelain_blame() {
        let out = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 1 1 2
author Jane Doe
author-mail <jane@example.com>
author-time 1700000000
author-tz +0000
summary Add the greeter
filename a.swift
\tline one
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 2 2
\tline two
0000000000000000000000000000000000000000 3 3 1
author Not Committed Yet
author-mail <not.committed.yet>
author-time 1800000000
summary Version of a.swift from a.swift
filename a.swift
\tnew line
";
        let blame = parse_porcelain(out);
        assert_eq!(blame.lines, vec![0, 0, 1]);
        let jane = blame.commit(1).unwrap();
        assert_eq!((jane.author.as_str(), jane.email.as_str(), jane.time), ("Jane Doe", "jane@example.com", 1_700_000_000));
        assert_eq!((jane.summary.as_str(), jane.short_sha(), jane.uncommitted), ("Add the greeter", "aaaaaaaa", false));
        assert!(blame.commit(2).unwrap().uncommitted);
        assert_eq!(blame.commit(3), None);
        let new = Blame::uncommitted(2);
        assert!(new.commit(1).is_some_and(|c| c.uncommitted && c.author == "Not Committed Yet"));
        assert_eq!(new.commit(2), None);
    }

    #[test]
    fn times() {
        let now = 1_000_000_000;
        assert_eq!(relative_time(now - 5, now), "just now");
        assert_eq!(relative_time(now - 60, now), "1 minute ago");
        assert_eq!(relative_time(now - 3 * 3600, now), "3 hours ago");
        assert_eq!(relative_time(now - 2 * 86_400, now), "2 days ago");
        assert_eq!(relative_time(now - 15 * 86_400, now), "2 weeks ago");
        assert_eq!(relative_time(now - 90 * 86_400, now), "3 months ago");
        assert_eq!(relative_time(now - 800 * 86_400, now), "2 years ago");
        assert_eq!(absolute_time(1_700_000_000), "2023-11-14 22:13 UTC");
    }
}

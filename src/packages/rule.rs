//! Versions and Xcode's dependency rules: Up to Next Major Version, Up to
//! Next Minor Version, Range of Versions, Exact Version, Branch and Commit.
//! A rule is written to `project.pbxproj` as a package reference's
//! `requirement`, passed to `swift package add-dependency`, and read back
//! from both.

use std::cmp::Ordering;
use std::fmt;

/// A semantic version from a release tag (`1.2.3`, `v1.2.3`, `1.2.3-beta.1`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl Version {
    /// Parses a tag the way SwiftPM reads them: an optional `v`, then two
    /// or three numbers (a missing patch is 0), then optional pre-release
    /// and build metadata.
    pub fn parse(tag: &str) -> Option<Self> {
        let s = tag.trim();
        let s = s.strip_prefix('v').unwrap_or(s);
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((core, pre)) if !pre.is_empty() => (core, Some(pre.to_string())),
            Some(_) => return None,
            None => (s, None),
        };
        let mut parts = core.split('.');
        let number = |p: Option<&str>| -> Option<u64> {
            let p = p?;
            (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())).then(|| p.parse().ok())?
        };
        let major = number(parts.next())?;
        let minor = number(parts.next())?;
        let patch = match parts.next() {
            Some(p) => number(Some(p))?,
            None => 0,
        };
        if parts.next().is_some() {
            return None;
        }
        Some(Self { major, minor, patch, pre })
    }

    pub fn is_prerelease(&self) -> bool {
        self.pre.is_some()
    }

    pub fn next_major(&self) -> Self {
        Self { major: self.major + 1, minor: 0, patch: 0, pre: None }
    }

    pub fn next_minor(&self) -> Self {
        Self { major: self.major, minor: self.minor + 1, patch: 0, pre: None }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)).then_with(|| {
            // A pre-release sorts before its release; pre-releases compare
            // by their dot-separated identifiers.
            match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => {
                    let mut a = a.split('.');
                    let mut b = b.split('.');
                    loop {
                        match (a.next(), b.next()) {
                            (None, None) => return Ordering::Equal,
                            (None, Some(_)) => return Ordering::Less,
                            (Some(_), None) => return Ordering::Greater,
                            (Some(x), Some(y)) => {
                                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                                    (Ok(x), Ok(y)) => x.cmp(&y),
                                    (Ok(_), Err(_)) => Ordering::Less,
                                    (Err(_), Ok(_)) => Ordering::Greater,
                                    (Err(_), Err(_)) => x.cmp(y),
                                };
                                if order != Ordering::Equal {
                                    return order;
                                }
                            }
                        }
                    }
                }
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A release tag and the version it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: Version,
}

/// The release tags among `tags`, newest first (pre-releases included).
pub fn releases(tags: impl IntoIterator<Item = String>) -> Vec<Release> {
    let mut out: Vec<Release> =
        tags.into_iter().filter_map(|tag| Some(Release { version: Version::parse(&tag)?, tag })).collect();
    out.sort_by(|a, b| b.version.cmp(&a.version).then_with(|| a.tag.len().cmp(&b.tag.len())));
    out.dedup_by(|a, b| a.version == b.version);
    out
}

/// The newest release that isn't a pre-release, else the newest one.
pub fn latest(releases: &[Release]) -> Option<&Release> {
    releases.iter().find(|r| !r.version.is_prerelease()).or(releases.first())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleKind {
    UpToNextMajor,
    UpToNextMinor,
    Range,
    Exact,
    Branch,
    Commit,
}

impl RuleKind {
    /// In the order of Xcode's Dependency Rule menu.
    pub const ALL: [RuleKind; 6] =
        [Self::UpToNextMajor, Self::UpToNextMinor, Self::Range, Self::Exact, Self::Branch, Self::Commit];

    pub fn label(self) -> &'static str {
        match self {
            Self::UpToNextMajor => "Up to Next Major Version",
            Self::UpToNextMinor => "Up to Next Minor Version",
            Self::Range => "Range of Versions",
            Self::Exact => "Exact Version",
            Self::Branch => "Branch",
            Self::Commit => "Commit",
        }
    }

    /// For the control channel: `major`, `minor`, `range`, `exact`,
    /// `branch`, `commit`, or the menu label.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        Self::ALL.into_iter().find(|k| k.label().eq_ignore_ascii_case(s)).or(match s {
            "major" => Some(Self::UpToNextMajor),
            "minor" => Some(Self::UpToNextMinor),
            "range" => Some(Self::Range),
            "exact" => Some(Self::Exact),
            "branch" => Some(Self::Branch),
            "commit" | "revision" => Some(Self::Commit),
            _ => None,
        })
    }

    pub fn uses_versions(self) -> bool {
        !matches!(self, Self::Branch | Self::Commit)
    }
}

/// A dependency rule. Versions are kept as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rule {
    UpToNextMajor(String),
    UpToNextMinor(String),
    /// From the first version up to, not including, the second.
    Range(String, String),
    Exact(String),
    Branch(String),
    Commit(String),
}

impl Rule {
    pub fn kind(&self) -> RuleKind {
        match self {
            Self::UpToNextMajor(_) => RuleKind::UpToNextMajor,
            Self::UpToNextMinor(_) => RuleKind::UpToNextMinor,
            Self::Range(..) => RuleKind::Range,
            Self::Exact(_) => RuleKind::Exact,
            Self::Branch(_) => RuleKind::Branch,
            Self::Commit(_) => RuleKind::Commit,
        }
    }

    /// The versions the rule allows, as `[lower, upper)` (`upper` is `None`
    /// for an exact version, which matches only `lower`).
    fn bounds(&self) -> Option<(Version, Option<Version>)> {
        match self {
            Self::UpToNextMajor(v) => Version::parse(v).map(|v| {
                let upper = v.next_major();
                (v, Some(upper))
            }),
            Self::UpToNextMinor(v) => Version::parse(v).map(|v| {
                let upper = v.next_minor();
                (v, Some(upper))
            }),
            Self::Range(from, to) => Some((Version::parse(from)?, Some(Version::parse(to)?))),
            Self::Exact(v) => Version::parse(v).map(|v| (v, None)),
            Self::Branch(_) | Self::Commit(_) => None,
        }
    }

    /// Whether the rule is complete enough to add: versions parse, a range
    /// isn't empty, a branch or commit is named.
    pub fn is_valid(&self) -> bool {
        match self {
            Self::Branch(name) => !name.trim().is_empty() && !name.contains(char::is_whitespace),
            Self::Commit(sha) => sha.len() >= 7 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
            Self::Range(..) => self.bounds().is_some_and(|(lower, upper)| upper.is_some_and(|u| lower < u)),
            _ => self.bounds().is_some(),
        }
    }

    pub fn allows(&self, version: &Version) -> bool {
        match self.bounds() {
            Some((lower, None)) => *version == lower,
            Some((lower, Some(upper))) => {
                // As in SwiftPM, a range only takes pre-releases when one of
                // its bounds is a pre-release.
                let pre_ok = lower.is_prerelease() || upper.is_prerelease();
                lower <= *version && *version < upper && (pre_ok || !version.is_prerelease())
            }
            None => false,
        }
    }

    /// The newest release the rule allows.
    pub fn pick<'a>(&self, releases: &'a [Release]) -> Option<&'a Release> {
        releases.iter().find(|r| self.allows(&r.version))
    }

    /// The upper bound Xcode shows next to the version field (`< 2.0.0`).
    pub fn upper_bound(&self) -> Option<Version> {
        match self {
            Self::UpToNextMajor(_) | Self::UpToNextMinor(_) => self.bounds().and_then(|(_, upper)| upper),
            _ => None,
        }
    }

    /// One line for the navigator and the sheet: `1.2.0 – Next Major`.
    pub fn summary(&self) -> String {
        match self {
            Self::UpToNextMajor(v) => format!("{v} – Next Major"),
            Self::UpToNextMinor(v) => format!("{v} – Next Minor"),
            Self::Range(from, to) => format!("{from} – {to}"),
            Self::Exact(v) => format!("Exactly {v}"),
            Self::Branch(b) => format!("Branch: {b}"),
            Self::Commit(c) => format!("Commit: {}", &c[..c.len().min(7)]),
        }
    }

    /// The `requirement` dictionary of an `XCRemoteSwiftPackageReference`,
    /// in Xcode's key order.
    pub fn pbxproj_fields(&self) -> Vec<(&'static str, String)> {
        match self {
            Self::UpToNextMajor(v) => vec![("kind", "upToNextMajorVersion".into()), ("minimumVersion", v.clone())],
            Self::UpToNextMinor(v) => vec![("kind", "upToNextMinorVersion".into()), ("minimumVersion", v.clone())],
            Self::Range(from, to) => vec![
                ("kind", "versionRange".into()),
                ("maximumVersion", to.clone()),
                ("minimumVersion", from.clone()),
            ],
            Self::Exact(v) => vec![("kind", "exactVersion".into()), ("version", v.clone())],
            Self::Branch(b) => vec![("branch", b.clone()), ("kind", "branch".into())],
            Self::Commit(c) => vec![("kind", "revision".into()), ("revision", c.clone())],
        }
    }

    /// Reads a `requirement` dictionary (`get` looks up its keys).
    pub fn from_pbxproj<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Option<Self> {
        let s = |key: &str| get(key).map(str::to_string);
        Some(match get("kind")? {
            "upToNextMajorVersion" => Self::UpToNextMajor(s("minimumVersion")?),
            "upToNextMinorVersion" => Self::UpToNextMinor(s("minimumVersion")?),
            "versionRange" => Self::Range(s("minimumVersion")?, s("maximumVersion")?),
            "exactVersion" => Self::Exact(s("version")?),
            "branch" => Self::Branch(s("branch")?),
            "revision" => Self::Commit(s("revision")?),
            _ => return None,
        })
    }

    /// `swift package add-dependency` options.
    pub fn swiftpm_args(&self) -> Vec<String> {
        let args: Vec<&str> = match self {
            Self::UpToNextMajor(v) => vec!["--from", v],
            Self::UpToNextMinor(v) => vec!["--up-to-next-minor-from", v],
            Self::Range(from, to) => vec!["--from", from, "--to", to],
            Self::Exact(v) => vec!["--exact", v],
            Self::Branch(b) => vec!["--branch", b],
            Self::Commit(c) => vec!["--revision", c],
        };
        args.into_iter().map(str::to_string).collect()
    }

    /// The `requirement` of a dependency in `swift package dump-package`.
    pub fn from_dump(requirement: &serde_json::Value) -> Option<Self> {
        let first = |key: &str| requirement.get(key)?.as_array()?.first().cloned();
        if let Some(range) = first("range") {
            let lower = range.get("lowerBound")?.as_str()?.to_string();
            let upper = range.get("upperBound")?.as_str()?.to_string();
            let (l, u) = (Version::parse(&lower)?, Version::parse(&upper)?);
            return Some(if u == l.next_major() {
                Self::UpToNextMajor(lower)
            } else if u == l.next_minor() {
                Self::UpToNextMinor(lower)
            } else {
                Self::Range(lower, upper)
            });
        }
        if let Some(v) = first("exact") {
            return Some(Self::Exact(v.as_str()?.to_string()));
        }
        if let Some(b) = first("branch") {
            return Some(Self::Branch(b.as_str()?.to_string()));
        }
        if let Some(r) = first("revision") {
            return Some(Self::Commit(r.as_str()?.to_string()));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parses_tags() {
        assert_eq!(v("1.2.3"), Version { major: 1, minor: 2, patch: 3, pre: None });
        assert_eq!(v("v2.0"), Version { major: 2, minor: 0, patch: 0, pre: None });
        assert_eq!(v("1.0.0-beta.2+exp").pre.as_deref(), Some("beta.2"));
        for bad in ["", "1", "1.x.0", "1.2.3.4", "release-1", "1.2.3-", "01.a"] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
        assert_eq!(v("1.0.0-rc.1").to_string(), "1.0.0-rc.1");
    }

    #[test]
    fn orders_like_semver() {
        let mut list = vec![v("1.0.0"), v("1.0.0-alpha"), v("1.0.0-alpha.1"), v("1.0.0-beta"), v("0.9.12"), v("1.0.0-alpha.beta")];
        list.sort();
        let names: Vec<String> = list.iter().map(Version::to_string).collect();
        assert_eq!(names, ["0.9.12", "1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta", "1.0.0"]);
    }

    #[test]
    fn lists_releases_newest_first() {
        let tags = ["1.0.0", "v1.1.0", "2.0.0-beta.1", "not-a-version", "1.10.0", "1.2.0"].map(String::from);
        let list = releases(tags);
        let names: Vec<&str> = list.iter().map(|r| r.tag.as_str()).collect();
        assert_eq!(names, ["2.0.0-beta.1", "1.10.0", "1.2.0", "v1.1.0", "1.0.0"]);
        assert_eq!(latest(&list).unwrap().tag, "1.10.0");
    }

    #[test]
    fn rules_pick_versions() {
        let list = releases(["1.0.0", "1.1.0", "1.1.5", "1.2.0", "2.0.0", "2.1.0-beta"].map(String::from));
        let pick = |rule: Rule| rule.pick(&list).map(|r| r.tag.clone());
        assert_eq!(pick(Rule::UpToNextMajor("1.0.0".into())).as_deref(), Some("1.2.0"));
        assert_eq!(pick(Rule::UpToNextMinor("1.1.0".into())).as_deref(), Some("1.1.5"));
        assert_eq!(pick(Rule::Range("1.0.0".into(), "1.2.0".into())).as_deref(), Some("1.1.5"));
        assert_eq!(pick(Rule::Exact("1.1.0".into())).as_deref(), Some("1.1.0"));
        assert_eq!(pick(Rule::UpToNextMajor("2.0.0".into())).as_deref(), Some("2.0.0"));
        assert_eq!(pick(Rule::UpToNextMajor("3.0.0".into())), None);
        assert_eq!(pick(Rule::Range("2.1.0-alpha".into(), "3.0.0".into())).as_deref(), Some("2.1.0-beta"));
        assert_eq!(Rule::UpToNextMajor("1.4.2".into()).upper_bound(), Some(v("2.0.0")));
        assert_eq!(Rule::UpToNextMinor("1.4.2".into()).upper_bound(), Some(v("1.5.0")));
    }

    #[test]
    fn validates_rules() {
        assert!(Rule::UpToNextMajor("1.0.0".into()).is_valid());
        assert!(!Rule::UpToNextMajor("one".into()).is_valid());
        assert!(!Rule::Range("2.0.0".into(), "1.0.0".into()).is_valid());
        assert!(Rule::Branch("main".into()).is_valid());
        assert!(!Rule::Branch("".into()).is_valid());
        assert!(Rule::Commit("e6d008ffd23557380a135562ee0a7ebb9c773afe".into()).is_valid());
        assert!(!Rule::Commit("main".into()).is_valid());
    }

    #[test]
    fn round_trips_pbxproj_requirements() {
        let rules = [
            Rule::UpToNextMajor("1.0.0".into()),
            Rule::UpToNextMinor("1.1.0".into()),
            Rule::Range("1.0.0".into(), "1.5.0".into()),
            Rule::Exact("1.2.0".into()),
            Rule::Branch("develop".into()),
            Rule::Commit("e6d008f".into()),
        ];
        for rule in rules {
            let fields = rule.pbxproj_fields();
            let read = Rule::from_pbxproj(|key| fields.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str()));
            assert_eq!(read.as_ref(), Some(&rule));
        }
    }

    #[test]
    fn reads_dump_package_requirements() {
        let json = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
        assert_eq!(
            Rule::from_dump(&json(r#"{"range":[{"lowerBound":"1.0.0","upperBound":"2.0.0"}]}"#)),
            Some(Rule::UpToNextMajor("1.0.0".into()))
        );
        assert_eq!(
            Rule::from_dump(&json(r#"{"range":[{"lowerBound":"1.2.0","upperBound":"1.3.0"}]}"#)),
            Some(Rule::UpToNextMinor("1.2.0".into()))
        );
        assert_eq!(
            Rule::from_dump(&json(r#"{"range":[{"lowerBound":"1.0.0","upperBound":"1.5.0"}]}"#)),
            Some(Rule::Range("1.0.0".into(), "1.5.0".into()))
        );
        assert_eq!(Rule::from_dump(&json(r#"{"exact":["1.2.0"]}"#)), Some(Rule::Exact("1.2.0".into())));
        assert_eq!(Rule::from_dump(&json(r#"{"branch":["main"]}"#)), Some(Rule::Branch("main".into())));
        assert_eq!(Rule::from_dump(&json(r#"{"revision":["abc1234"]}"#)), Some(Rule::Commit("abc1234".into())));
    }

    #[test]
    fn swiftpm_options() {
        assert_eq!(Rule::Range("1.0.0".into(), "1.5.0".into()).swiftpm_args(), ["--from", "1.0.0", "--to", "1.5.0"]);
        assert_eq!(Rule::UpToNextMinor("1.1.0".into()).swiftpm_args(), ["--up-to-next-minor-from", "1.1.0"]);
        assert_eq!(RuleKind::parse("Exact Version"), Some(RuleKind::Exact));
        assert_eq!(RuleKind::parse("commit"), Some(RuleKind::Commit));
    }
}

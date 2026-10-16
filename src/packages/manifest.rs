//! `Package.swift`: the dependencies a package declares, and removing one.
//! Adding goes through `swift package add-dependency` and
//! `add-target-dependency`; SwiftPM has no command to remove a dependency,
//! so this removes its `.package(…)` entry and the `.product(…, package:)`
//! entries that use it, leaving the rest of the file as it was.

use std::ops::Range;

use anyhow::{Result, bail};

use super::rule::{Rule, Version};
use super::resolved;

/// A `.package(…)` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declared {
    /// `url:` (or a registry `id:`), else `None` for `path:` packages.
    pub url: Option<String>,
    pub path: Option<String>,
    pub rule: Option<Rule>,
    /// The entry, from `.package` through its closing parenthesis.
    pub span: Range<usize>,
}

impl Declared {
    pub fn location(&self) -> &str {
        self.url.as_deref().or(self.path.as_deref()).unwrap_or("")
    }

    pub fn identity(&self) -> String {
        resolved::identity(self.location())
    }
}

/// Byte ranges of code (outside strings and comments) in Swift source.
fn code_mask(text: &str) -> Vec<bool> {
    let bytes = text.as_bytes();
    let mut mask = vec![true; bytes.len()];
    let mut i = 0;
    while i < bytes.len() {
        let rest = &bytes[i..];
        let start = i;
        if rest.starts_with(b"//") {
            i += rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
        } else if rest.starts_with(b"/*") {
            let mut depth = 0;
            while i < bytes.len() {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if rest[0] == b'"' || (rest[0] == b'#' && rest.iter().skip_while(|&&b| b == b'#').next() == Some(&b'"')) {
            let hashes = rest.iter().take_while(|&&b| b == b'#').count();
            let quote = &rest[hashes..];
            let delimiter = if quote.starts_with(b"\"\"\"") { "\"\"\"" } else { "\"" };
            let close = format!("{delimiter}{}", "#".repeat(hashes));
            i += hashes + delimiter.len();
            while i < bytes.len() {
                if hashes == 0 && bytes[i] == b'\\' {
                    i += 2;
                } else if bytes[i..].starts_with(close.as_bytes()) {
                    i += close.len();
                    break;
                } else {
                    i += 1;
                }
            }
        } else {
            i += 1;
            continue;
        }
        let end = i.min(bytes.len());
        mask[start..end].iter_mut().for_each(|m| *m = false);
    }
    mask
}

/// The offset just past the parenthesis closing the one at `open`.
fn closing_paren(text: &str, mask: &[bool], open: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, b) in text.bytes().enumerate().skip(open) {
        if !mask[i] {
            continue;
        }
        match b {
            b'(' | b'[' => depth += 1,
            b')' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Call sites of `.<name>(` in code, with their spans.
fn calls(text: &str, mask: &[bool], name: &str) -> Vec<Range<usize>> {
    let needle = format!(".{name}(");
    text.match_indices(&needle)
        .filter(|(at, _)| mask[*at])
        .filter_map(|(at, _)| Some(at..closing_paren(text, mask, at + needle.len() - 1)?))
        .collect()
}

/// The string literal after `label:` among a call's arguments.
fn string_argument(args: &str, label: &str) -> Option<String> {
    let mask = code_mask(args);
    let needle = format!("{label}:");
    let at = args
        .match_indices(&needle)
        .find(|(at, _)| mask[*at] && (*at == 0 || !args.as_bytes()[at - 1].is_ascii_alphanumeric()))?
        .0;
    let rest = args[at + needle.len()..].trim_start();
    let rest = rest.strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_string())
}

/// All string literals in `text`, in order.
fn strings(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = text;
    while let Some(start) = rest.find('"') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('"') else { break };
        out.push(after[..end].to_string());
        rest = &after[end + 1..];
    }
    out
}

fn rule_of(args: &str) -> Option<Rule> {
    let s = |label| string_argument(args, label);
    if let Some(v) = s("exact") {
        return Some(Rule::Exact(v));
    }
    if let Some(b) = s("branch") {
        return Some(Rule::Branch(b));
    }
    if let Some(r) = s("revision") {
        return Some(Rule::Commit(r));
    }
    if args.contains(".upToNextMinor") {
        return s("from").map(Rule::UpToNextMinor);
    }
    if let Some(v) = s("from") {
        return Some(Rule::UpToNextMajor(v));
    }
    // `"1.0.0"..<"2.0.0"` (or `...`).
    if let Some(at) = args.find("..<").or_else(|| args.find("...")) {
        let lower = strings(&args[..at]).pop()?;
        let upper = strings(&args[at..]).into_iter().next()?;
        // `add-dependency --up-to-next-minor-from` writes a range.
        let next = |v: &str| Version::parse(v);
        return Some(match (next(&lower), next(&upper)) {
            (Some(l), Some(u)) if u == l.next_major() => Rule::UpToNextMajor(lower),
            (Some(l), Some(u)) if u == l.next_minor() => Rule::UpToNextMinor(lower),
            _ => Rule::Range(lower, upper),
        });
    }
    None
}

/// The `.package(…)` dependencies of a manifest.
pub fn declared(text: &str) -> Vec<Declared> {
    let mask = code_mask(text);
    calls(text, &mask, "package")
        .into_iter()
        .filter_map(|span| {
            let args = &text[span.start + ".package(".len()..span.end - 1];
            let url = string_argument(args, "url").or_else(|| string_argument(args, "id"));
            let path = string_argument(args, "path");
            if url.is_none() && path.is_none() {
                return None;
            }
            Some(Declared { rule: url.as_ref().and_then(|_| rule_of(args)), url, path, span })
        })
        .collect()
}

/// What to delete to remove a list element at `span`: the element and its
/// comma, and its line when nothing else is on it.
fn element_removal(text: &str, span: Range<usize>) -> Range<usize> {
    let bytes = text.as_bytes();
    let mut end = span.end;
    while end < bytes.len() && (bytes[end] == b' ' || bytes[end] == b'\t') {
        end += 1;
    }
    let had_comma = bytes.get(end) == Some(&b',');
    if had_comma {
        end += 1;
    }
    let line_start = text[..span.start].rfind('\n').map_or(0, |n| n + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |n| end + n);
    if text[line_start..span.start].trim().is_empty() && text[end..line_end].trim().is_empty() {
        return line_start..(line_end + 1).min(text.len());
    }
    if had_comma {
        // `a, b, c`: take the space after the comma too.
        while end < bytes.len() && bytes[end] == b' ' {
            end += 1;
        }
        return span.start..end;
    }
    // The last element of a one-line list: take the comma before it.
    let mut start = span.start;
    let before = text[..start].trim_end_matches([' ', '\t']);
    if before.ends_with(',') {
        start = before.len() - 1;
    }
    start..span.end
}

/// The manifest without dependency `identity` (matched by the identity of
/// its URL or path) and the products targets use from it.
pub fn remove_dependency(text: &str, identity: &str) -> Result<String> {
    let mask = code_mask(text);
    let mut removals: Vec<Range<usize>> = vec![];
    let packages: Vec<Declared> = declared(text).into_iter().filter(|d| d.identity().eq_ignore_ascii_case(identity)).collect();
    if packages.is_empty() {
        bail!("Package.swift doesn’t depend on “{identity}”");
    }
    removals.extend(packages.iter().map(|d| element_removal(text, d.span.clone())));
    for span in calls(text, &mask, "product") {
        let args = &text[span.start + ".product(".len()..span.end - 1];
        if string_argument(args, "package").is_some_and(|p| resolved::identity(&p).eq_ignore_ascii_case(identity)) {
            removals.push(element_removal(text, span));
        }
    }
    removals.sort_by_key(|r| r.start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for range in removals {
        if range.start < at {
            continue;
        }
        out.push_str(&text[at..range.start]);
        at = range.end;
    }
    out.push_str(&text[at..]);
    // A `dependencies: []` left empty stays, as SwiftPM's tools leave it.
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"// swift-tools-version: 6.0
import PackageDescription

// .package(url: "https://example.com/commented-out.git", from: "1.0.0"),
let package = Package(
    name: "ZKit",
    dependencies: [
        .package(url: "https://github.com/apple/swift-argument-parser.git", from: "1.3.0"),
        .package(url: "file:///tmp/zcode-tally/Tally.git", "1.0.0"..<"1.5.0"),
        .package(url: "https://github.com/apple/swift-collections", .upToNextMinor(from: "1.1.0")),
        .package(path: "../LocalKit"),
        .package(url: "https://example.com/pinned.git", exact: "2.0.0"),
        .package(url: "https://example.com/edge.git", branch: "main"),
        .package(url: "https://example.com/minor.git", "1.1.0" ..< "1.2.0"),
    ],
    targets: [
        .target(name: "ZKit", dependencies: [
            .product(name: "Tally", package: "Tally"),
            .product(name: "TallyFormat", package: "tally"),
            .product(name: "ArgumentParser", package: "swift-argument-parser"),
        ]),
        .executableTarget(name: "zhello", dependencies: ["ZKit", .product(name: "Tally", package: "Tally")]),
        .testTarget(name: "ZKitTests", dependencies: [.product(name: "Tally", package: "Tally")]),
    ]
)
"#;

    #[test]
    fn reads_declared_dependencies() {
        let deps = declared(MANIFEST);
        let summary: Vec<(String, Option<Rule>)> = deps.iter().map(|d| (d.identity(), d.rule.clone())).collect();
        assert_eq!(
            summary,
            [
                ("swift-argument-parser".to_string(), Some(Rule::UpToNextMajor("1.3.0".into()))),
                ("tally".to_string(), Some(Rule::Range("1.0.0".into(), "1.5.0".into()))),
                ("swift-collections".to_string(), Some(Rule::UpToNextMinor("1.1.0".into()))),
                ("localkit".to_string(), None),
                ("pinned".to_string(), Some(Rule::Exact("2.0.0".into()))),
                ("edge".to_string(), Some(Rule::Branch("main".into()))),
                ("minor".to_string(), Some(Rule::UpToNextMinor("1.1.0".into()))),
            ]
        );
        assert_eq!(deps[3].path.as_deref(), Some("../LocalKit"));
    }

    #[test]
    fn removes_a_dependency_and_its_products() {
        let out = remove_dependency(MANIFEST, "tally").unwrap();
        assert!(!out.contains("Tally.git"), "{out}");
        assert!(!out.contains("package: \"Tally\"") && !out.contains("package: \"tally\""), "{out}");
        assert!(out.contains("dependencies: [\"ZKit\"]),"), "{out}");
        assert!(out.contains(".testTarget(name: \"ZKitTests\", dependencies: []),"), "{out}");
        assert!(out.contains("// .package(url: \"https://example.com/commented-out.git\""), "comments are left alone");
        assert!(out.contains("            .product(name: \"ArgumentParser\", package: \"swift-argument-parser\"),\n        ]),"), "{out}");
        // Lines that didn't mention the package are untouched.
        for line in MANIFEST.lines().filter(|l| !l.contains("Tally") && !l.contains("tally")) {
            assert!(out.contains(line), "{line}");
        }
        assert_eq!(declared(&out).len(), 6);
        assert!(remove_dependency(MANIFEST, "missing").is_err());
    }

    #[test]
    fn removes_from_the_toolchains_formatting() {
        // What `swift package add-target-dependency` writes.
        let text = "        .target(name: \"ZKit\",dependencies: [\n    .product(name: \"Tally\", package: \"Tally\"),]),\n";
        let manifest = format!("let p = Package(dependencies: [\n        .package(url: \"file:///tmp/Tally.git\", from: \"1.0.0\"),\n    ], targets: [\n{text}])\n");
        let out = remove_dependency(&manifest, "tally").unwrap();
        assert!(out.contains(".target(name: \"ZKit\",dependencies: [\n    ]),"), "{out}");
        assert!(out.contains("Package(dependencies: [\n    ], targets"), "{out}");
    }

    #[test]
    fn skips_strings_and_comments() {
        let text = r#"let s = ".package(url: \"x\")" /* .package(url: "y") */ ; let p = [.package(url: "z", from: "1.0.0")]"#;
        let deps = declared(text);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].url.as_deref(), Some("z"));
    }
}

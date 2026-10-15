//! Turning xcodebuild / swift build output into issues, test results and a
//! readable, Xcode-style activity log.

use std::path::PathBuf;
use std::sync::OnceLock;

use regex::Regex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl Severity {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "error" | "fatal error" => Some(Self::Error),
            "warning" => Some(Self::Warning),
            "note" | "remark" => Some(Self::Note),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Issue {
    pub severity: Severity,
    pub message: String,
    pub path: Option<PathBuf>,
    /// 1-based line and column (as printed by the compiler).
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub source: IssueSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IssueSource {
    Build,
    Live,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestStatus {
    Passed,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TestResult {
    pub name: String,
    pub status: TestStatus,
    pub duration: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineStyle {
    Plain,
    /// A build step ("Compile Foo.swift").
    Step,
    Error,
    Warning,
    Note,
    Success,
    Failure,
}

#[derive(Clone, Debug)]
pub struct LogLine {
    pub raw: String,
    /// Condensed, Xcode-style text; `None` when the line is noise that only
    /// appears in the raw log.
    pub pretty: Option<String>,
    pub style: LineStyle,
}

/// Everything learned from a single output line.
#[derive(Debug, Default)]
pub struct Parsed {
    pub issue: Option<Issue>,
    pub test: Option<TestResult>,
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid regex"))
}

pub fn parse_issue(line: &str) -> Option<Issue> {
    static LOCATED: OnceLock<Regex> = OnceLock::new();
    static LINE_ONLY: OnceLock<Regex> = OnceLock::new();
    static GENERIC: OnceLock<Regex> = OnceLock::new();
    static TARGET_SUFFIX: OnceLock<Regex> = OnceLock::new();
    // xcodebuild repeats each error in its summary with a target suffix.
    let line = line.trim_end();
    let stripped = re(&TARGET_SUFFIX, r" \(in target '[^']*' from project '[^']*'\)$").replace(line, "");
    let line = stripped.as_ref();
    if let Some(c) = re(
        &LOCATED,
        r"^(/[^:]+):(\d+):(\d+): (error|warning|note|fatal error|remark): (.+)$",
    )
    .captures(line)
    {
        return Some(Issue {
            severity: Severity::parse(&c[4])?,
            message: c[5].to_string(),
            path: Some(PathBuf::from(&c[1])),
            line: c[2].parse().ok(),
            column: c[3].parse().ok(),
            source: IssueSource::Build,
        });
    }
    if let Some(c) = re(&LINE_ONLY, r"^(/[^:]+):(\d+): (error|warning|note): (.+)$").captures(line)
    {
        return Some(Issue {
            severity: Severity::parse(&c[3])?,
            message: c[4].to_string(),
            path: Some(PathBuf::from(&c[1])),
            line: c[2].parse().ok(),
            column: None,
            source: IssueSource::Build,
        });
    }
    // Swift Testing: "✘ Test foo() recorded an issue at File.swift:8:9: Expectation failed: …".
    // Only the file name is printed; the workspace resolves it to a path.
    static TEST_ISSUE: OnceLock<Regex> = OnceLock::new();
    if let Some(c) = re(&TEST_ISSUE, r"recorded an issue at ([^:/]+):(\d+):(\d+): (.+)$").captures(line) {
        return Some(Issue {
            severity: Severity::Error,
            message: c[4].to_string(),
            path: Some(PathBuf::from(&c[1])),
            line: c[2].parse().ok(),
            column: c[3].parse().ok(),
            source: IssueSource::Build,
        });
    }
    // xcodebuild's "Testing failed:" summary: "\tApp (123) encountered an error (...)".
    if line.starts_with('\t') && line.contains(" encountered an error ") {
        return Some(Issue {
            severity: Severity::Error,
            message: line.trim().to_string(),
            path: None,
            line: None,
            column: None,
            source: IssueSource::Build,
        });
    }
    if let Some(c) = re(
        &GENERIC,
        r"^(?:([\w.\-/ ]+): )?(error|warning|fatal error): (.+)$",
    )
    .captures(line)
    {
        let tool = c.get(1).map(|m| m.as_str());
        // Lines like "    error: ..." inside a test failure are handled by the
        // test parser; generic errors need a tool prefix or start the line.
        let message = match tool {
            Some(tool) => format!("{tool}: {}", &c[3]),
            None => c[3].to_string(),
        };
        return Some(Issue {
            severity: Severity::parse(&c[2])?,
            message,
            path: None,
            line: None,
            column: None,
            source: IssueSource::Build,
        });
    }
    None
}

pub fn parse_test(line: &str) -> Option<TestResult> {
    static XCTEST: OnceLock<Regex> = OnceLock::new();
    static SWIFT_TESTING: OnceLock<Regex> = OnceLock::new();
    let line = line.trim();
    // XCTest: Test Case '-[Mod.Tests testFoo]' passed (0.001 seconds).
    //         Test case 'Tests.testFoo()' failed on 'iPhone' (0.002 seconds)
    if let Some(c) = re(
        &XCTEST,
        r"^Test [Cc]ase '(.+?)' (passed|failed|skipped)(?: on '.*?')? \((\d+\.\d+) seconds\)",
    )
    .captures(line)
    {
        let name = c[1]
            .trim_start_matches("-[")
            .trim_end_matches(']')
            .replace(' ', ".");
        return Some(TestResult {
            name,
            status: match &c[2] {
                "passed" => TestStatus::Passed,
                "failed" => TestStatus::Failed,
                _ => TestStatus::Skipped,
            },
            duration: c[3].parse().ok(),
        });
    }
    // Swift Testing: ✔ Test foo() passed after 0.001 seconds.
    //                ✘ Test foo() failed after 0.002 seconds with 1 issue.
    //                ➜ Test foo() skipped.
    if let Some(c) = re(
        &SWIFT_TESTING,
        r#"^[^\s]+ Test (?:"[^"]*" \()?([^\s"]+?\))\)? (passed|failed|skipped)(?: after (\d+\.\d+) seconds)?"#,
    )
    .captures(line)
    {
        return Some(TestResult {
            name: c[1].to_string(),
            status: match &c[2] {
                "passed" => TestStatus::Passed,
                "failed" => TestStatus::Failed,
                _ => TestStatus::Skipped,
            },
            duration: c.get(3).and_then(|m| m.as_str().parse().ok()),
        });
    }
    None
}

pub fn parse_line(line: &str) -> Parsed {
    Parsed {
        issue: parse_issue(line),
        test: parse_test(line),
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn unescape(s: &str) -> String {
    s.replace("\\ ", " ")
}

fn target_suffix(line: &str) -> String {
    static TARGET: OnceLock<Regex> = OnceLock::new();
    re(&TARGET, r"\(in target '([^']+)'")
        .captures(line)
        .map(|c| format!("  ({})", &c[1]))
        .unwrap_or_default()
}

/// Condenses one raw line into an Xcode-style activity entry.
pub fn prettify(raw: &str) -> LogLine {
    let line = raw.trim_end();
    let step = |text: String| LogLine {
        raw: raw.to_string(),
        pretty: Some(text),
        style: LineStyle::Step,
    };
    let hidden = || LogLine {
        raw: raw.to_string(),
        pretty: None,
        style: LineStyle::Plain,
    };

    if let Some(issue) = parse_issue(line) {
        let style = match issue.severity {
            Severity::Error => LineStyle::Error,
            Severity::Warning => LineStyle::Warning,
            Severity::Note => LineStyle::Note,
        };
        let location = match (&issue.path, issue.line) {
            (Some(path), Some(l)) => format!(
                "{}:{}: ",
                path.file_name().unwrap_or_default().to_string_lossy(),
                l
            ),
            _ => String::new(),
        };
        return LogLine {
            raw: raw.to_string(),
            pretty: Some(format!("{location}{}", issue.message)),
            style,
        };
    }
    if line.starts_with("** ") && line.ends_with(" **") {
        let ok = line.contains("SUCCEEDED");
        return LogLine {
            raw: raw.to_string(),
            pretty: Some(line.trim_matches('*').trim().to_string()),
            style: if ok { LineStyle::Success } else { LineStyle::Failure },
        };
    }
    if let Some(test) = parse_test(line) {
        let (mark, style) = match test.status {
            TestStatus::Passed => ("✓", LineStyle::Success),
            TestStatus::Failed => ("✗", LineStyle::Failure),
            TestStatus::Skipped => ("–", LineStyle::Note),
        };
        return LogLine {
            raw: raw.to_string(),
            pretty: Some(format!(
                "{mark} {}{}",
                test.name,
                test.duration.map(|d| format!("  {d:.3}s")).unwrap_or_default()
            )),
            style,
        };
    }

    let mut words = line.splitn(2, ' ');
    let task = words.next().unwrap_or("");
    let rest = words.next().unwrap_or("");
    let target = target_suffix(line);
    match task {
        "SwiftCompile" => {
            // Each file appears twice; keep the "Compiling\ Foo.swift" form.
            if let Some(idx) = rest.find("Compiling\\ ") {
                let name = rest[idx + "Compiling\\ ".len()..]
                    .split(" /")
                    .next()
                    .unwrap_or("");
                step(format!("Compile {}{target}", unescape(name)))
            } else {
                hidden()
            }
        }
        "CompileC" => {
            let src = rest.split(' ').nth(1).unwrap_or("");
            step(format!("Compile {}{target}", file_name(src)))
        }
        "Ld" => step(format!(
            "Link {}{target}",
            file_name(rest.split(' ').next().unwrap_or(""))
        )),
        "CodeSign" => step(format!(
            "Sign {}{target}",
            file_name(&unescape(rest.split(" (in").next().unwrap_or("")))
        )),
        "CompileAssetCatalog" | "CompileAssetCatalogVariant" => {
            step(format!("Compile asset catalog{target}"))
        }
        "CompileStoryboard" | "CompileXIB" => step(format!(
            "Compile {}{target}",
            file_name(rest.split(' ').next().unwrap_or(""))
        )),
        "PhaseScriptExecution" => step(format!(
            "Run script {}{target}",
            unescape(rest.split(" /").next().unwrap_or(""))
        )),
        "ProcessInfoPlistFile" => step(format!("Process Info.plist{target}")),
        "GenerateDSYMFile" => step(format!("Generate dSYM{target}")),
        "Building" if rest.starts_with("for ") => step(line.to_string()),
        "Resolve" if rest.starts_with("Package Graph") => step(line.to_string()),
        // swift build: "[3/12] Compiling Foo Bar.swift"
        _ if line.starts_with('[') && line.contains("] ") => {
            let text = line.split_once("] ").map(|x| x.1).unwrap_or(line);
            step(text.to_string())
        }
        _ if line.starts_with("Build complete!") || line.starts_with("Build of product") => {
            LogLine {
                raw: raw.to_string(),
                pretty: Some(line.to_string()),
                style: LineStyle::Success,
            }
        }
        _ => hidden(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_located_errors() {
        let issue = parse_issue(
            "/Users/me/App/ContentView.swift:12:5: error: cannot find 'foo' in scope",
        )
        .unwrap();
        assert_eq!(issue.severity, Severity::Error);
        assert_eq!(issue.line, Some(12));
        assert_eq!(issue.column, Some(5));
        assert_eq!(issue.message, "cannot find 'foo' in scope");
    }

    #[test]
    fn parses_swift_testing_issues() {
        let issue = parse_issue("✘ Test incrementAddsOne() recorded an issue at CounterTests.swift:8:9: Expectation failed: counter.count == 2").unwrap();
        assert_eq!(issue.path.as_deref(), Some(std::path::Path::new("CounterTests.swift")));
        assert_eq!((issue.line, issue.column), (Some(8), Some(9)));
        assert_eq!(issue.message, "Expectation failed: counter.count == 2");
    }

    #[test]
    fn parses_test_runner_errors() {
        let issue = parse_issue("\tHelloZ (32433) encountered an error (The test runner hung before establishing connection.)").unwrap();
        assert_eq!(issue.severity, Severity::Error);
        assert!(parse_test("✔ Test run with 2 tests in 0 suites passed after 0.001 seconds.").is_none());
    }

    #[test]
    fn strips_target_suffix() {
        let a = parse_issue("/p/A.swift:8:1: error: Bad thing").unwrap();
        let b = parse_issue("/p/A.swift:8:1: error: Bad thing (in target 'App' from project 'App')").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn parses_tool_errors() {
        let issue = parse_issue("xcodebuild: error: Unable to find a destination").unwrap();
        assert_eq!(issue.path, None);
        assert!(issue.message.starts_with("xcodebuild: "));
        assert!(parse_issue("note: Building targets in dependency order").is_none());
    }

    #[test]
    fn parses_tests() {
        let t = parse_test("Test Case '-[HelloZTests.CounterTests testIncrement]' passed (0.001 seconds).")
            .unwrap();
        assert_eq!(t.name, "HelloZTests.CounterTests.testIncrement");
        assert_eq!(t.status, TestStatus::Passed);
        let t = parse_test("✘ Test resetClearsCount() failed after 0.002 seconds with 1 issue.")
            .unwrap();
        assert_eq!(t.name, "resetClearsCount()");
        assert_eq!(t.status, TestStatus::Failed);
        let t = parse_test("✔ Test incrementAddsOne() passed after 0.001 seconds.").unwrap();
        assert_eq!(t.status, TestStatus::Passed);
    }

    #[test]
    fn prettifies_steps() {
        let line = r"SwiftCompile normal arm64 Compiling\ Counter.swift /p/HelloZ/Counter.swift (in target 'HelloZ' from project 'HelloZ')";
        assert_eq!(
            prettify(line).pretty.as_deref(),
            Some("Compile Counter.swift  (HelloZ)")
        );
        assert!(prettify("    cd /tmp").pretty.is_none());
        assert_eq!(prettify("** BUILD SUCCEEDED **").style, LineStyle::Success);
    }
}

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
    /// Package resolution (shown under Package Loading).
    Package,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestStatus {
    Passed,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TestResult {
    /// The test as the runner printed it: `testFoo`, `foo()`, or a Swift
    /// Testing display name in quotes (`"Adds one"`).
    pub name: String,
    /// Enclosing classes or suites, when the output names them.
    pub suites: Vec<String>,
    /// The test module, from XCTest's `-[Module.Class test]` form.
    pub module: Option<String>,
    pub status: TestStatus,
    pub duration: Option<f64>,
}

impl TestResult {
    /// `Class/test`, `Suite/Inner/test()` or just the test.
    pub fn full_name(&self) -> String {
        self.suites.iter().chain([&self.name]).cloned().collect::<Vec<_>>().join("/")
    }

    /// Whether this reports the same test as `other`. xcodebuild prints
    /// XCTest results twice, as `-[Module.Class test]` and `Class.test()`.
    pub fn same_test(&self, other: &TestResult) -> bool {
        let bare = |name: &str| name.strip_suffix("()").unwrap_or(name).to_string();
        bare(&self.name) == bare(&other.name)
            && self.suites == other.suites
            && (self.module == other.module || self.module.is_none() || other.module.is_none())
    }
}

/// A Swift Testing suite starting or finishing (`◇ Suite Foo started.`).
#[derive(Clone, Debug, PartialEq)]
pub enum SuiteEvent {
    Started(String),
    Finished(String),
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
    pub suite: Option<SuiteEvent>,
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid regex"))
}

pub fn parse_issue(line: &str) -> Option<Issue> {
    parse_issue_line(line).map(|issue| if is_signing_issue(&issue) { with_signing_hint(issue) } else { issue })
}

fn parse_issue_line(line: &str) -> Option<Issue> {
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
    // Project-level issues, signing above all: "/…/App.xcodeproj: error:
    // Signing for "App" requires a development team. …".
    static PROJECT: OnceLock<Regex> = OnceLock::new();
    if let Some(c) = re(&PROJECT, r"^(/[^:]+\.(?:xcodeproj|xcworkspace)): (error|warning|note): (.+)$").captures(line) {
        return Some(Issue {
            severity: Severity::parse(&c[2])?,
            message: c[3].to_string(),
            path: Some(PathBuf::from(&c[1])),
            line: None,
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

/// Whether an issue is about code signing or provisioning.
pub fn is_signing_issue(issue: &Issue) -> bool {
    issue.severity == Severity::Error
        && ["Signing for", "development team", "provisioning profile", "Provisioning profile", "No Account for Team", "signing certificate", "No profiles for"]
            .iter()
            .any(|needle| issue.message.contains(needle))
}

/// Xcode's signing errors point at its Signing & Capabilities editor; in
/// zcode that's the target's tab in the project settings (clicking the issue
/// opens it).
fn with_signing_hint(mut issue: Issue) -> Issue {
    const XCODE: &str = "Select a development team in the Signing & Capabilities editor.";
    const ZCODE: &str = "Choose the team in the target’s Signing & Capabilities tab in the project settings";
    issue.message = if issue.message.contains(XCODE) {
        issue.message.replace(XCODE, &format!("{ZCODE}."))
    } else {
        format!("{} {ZCODE}, and sign in to your Apple account in Xcode ▸ Settings ▸ Accounts.", issue.message)
    };
    issue
}

/// The target a signing error is about: `Signing for "App" …` or
/// `… (in target 'App' …)`.
pub fn signing_target(message: &str) -> Option<String> {
    let quoted = |start: &str, end: char| {
        let at = message.find(start)? + start.len();
        let len = message[at..].find(end)?;
        Some(message[at..at + len].to_string())
    };
    quoted("Signing for \"", '"').or_else(|| quoted("in target '", '\''))
}

fn status(word: &str) -> TestStatus {
    match word {
        "passed" => TestStatus::Passed,
        "failed" => TestStatus::Failed,
        _ => TestStatus::Skipped,
    }
}

/// Splits an xcodebuild test name: `Class.testFoo()` (XCTest),
/// `Suite/Inner/foo()` (Swift Testing) or `foo()`.
fn split_test_name(name: &str) -> (Vec<String>, String) {
    if name.contains('/') {
        let mut parts: Vec<String> = name.split('/').map(str::to_string).collect();
        let last = parts.pop().unwrap_or_default();
        return (parts, last);
    }
    let base = name.split('(').next().unwrap_or(name);
    match base.rfind('.') {
        Some(dot) => (vec![name[..dot].to_string()], name[dot + 1..].to_string()),
        None => (vec![], name.to_string()),
    }
}

pub fn parse_test(line: &str) -> Option<TestResult> {
    static XCTEST: OnceLock<Regex> = OnceLock::new();
    static SWIFT_TESTING: OnceLock<Regex> = OnceLock::new();
    let line = line.trim();
    // XCTest: Test Case '-[Mod.Tests testFoo]' passed (0.001 seconds).
    // xcodebuild: Test case 'Tests.testFoo()' failed on 'iPhone' (0.002 seconds)
    //             Test case 'Suite/foo()' passed on 'My Mac' (0.001 seconds)
    if let Some(c) = re(
        &XCTEST,
        r"^Test [Cc]ase '(.+?)' (passed|failed|skipped)(?: on '.*?')? \((\d+\.\d+) seconds\)",
    )
    .captures(line)
    {
        let (module, suites, name) = match c[1].strip_prefix("-[").and_then(|s| s.strip_suffix(']')) {
            Some(objc) => {
                let (class, method) = objc.split_once(' ').unwrap_or((objc, ""));
                match class.rsplit_once('.') {
                    Some((module, class)) => (Some(module.to_string()), vec![class.to_string()], method.to_string()),
                    None => (None, vec![class.to_string()], method.to_string()),
                }
            }
            None => {
                let (suites, name) = split_test_name(&c[1]);
                (None, suites, name)
            }
        };
        return Some(TestResult {
            name,
            suites,
            module,
            status: status(&c[2]),
            duration: c[3].parse().ok(),
        });
    }
    // Swift Testing: ✔ Test foo() passed after 0.001 seconds.
    //                ✘ Test "Adds one" failed after 0.002 seconds with 1 issue.
    //                ➜ Test foo() skipped.
    // Older releases print both names: Test "Adds one" (foo()) passed.
    if let Some(c) = re(
        &SWIFT_TESTING,
        r#"^[^\w\s']\S* Test (?:"[^"]*" \((\S+?\))\)|("[^"]*")|(\S+?\))) (passed|failed|skipped)(?: after (\d+\.\d+) seconds)?"#,
    )
    .captures(line)
    {
        let name = c.get(1).or(c.get(2)).or(c.get(3))?.as_str().to_string();
        return Some(TestResult {
            name,
            suites: vec![],
            module: None,
            status: status(&c[4]),
            duration: c.get(5).and_then(|m| m.as_str().parse().ok()),
        });
    }
    None
}

pub fn parse_suite_event(line: &str) -> Option<SuiteEvent> {
    static SUITE: OnceLock<Regex> = OnceLock::new();
    let c = re(&SUITE, r#"^[^\w\s']\S* Suite ("[^"]*"|\S+) (started|passed|failed|skipped)"#).captures(line.trim())?;
    let name = c[1].trim_matches('"').to_string();
    Some(match &c[2] {
        "started" => SuiteEvent::Started(name),
        _ => SuiteEvent::Finished(name),
    })
}

/// `line` without terminal escape sequences: newer `swift build`s color
/// their diagnostics even into a pipe, which would hide them from the
/// parsers below.
pub fn strip_ansi(line: &str) -> std::borrow::Cow<'_, str> {
    if !line.contains('\u{1b}') {
        return line.into();
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: parameters, then a final byte in @..~.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ESC \.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out.into()
}

pub fn parse_line(line: &str) -> Parsed {
    Parsed {
        issue: parse_issue(line),
        test: parse_test(line),
        suite: parse_suite_event(line),
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
                test.full_name(),
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
    fn colored_diagnostics_are_read() {
        let colored = "/p/Sources/ZKit/Broken.swift:1:19: \u{1b}[1;31merror: \u{1b}[1;39mcannot convert value\u{1b}[0;0m";
        let line = strip_ansi(colored);
        assert_eq!(line, "/p/Sources/ZKit/Broken.swift:1:19: error: cannot convert value");
        let issue = parse_line(&line).issue.unwrap();
        assert_eq!((issue.severity, issue.line, issue.column), (Severity::Error, Some(1), Some(19)));
        assert_eq!(strip_ansi("\u{1b}]8;;file:///a\u{7}link\u{1b}]8;;\u{1b}\\ plain"), "link plain");
        assert!(matches!(strip_ansi("plain"), std::borrow::Cow::Borrowed("plain")));
    }

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
    fn parses_signing_errors() {
        let issue = parse_issue(
            "/Users/me/App/App.xcodeproj: error: Signing for \"App\" requires a development team. Select a development team in the Signing & Capabilities editor. (in target 'App' from project 'App')",
        )
        .unwrap();
        assert_eq!(issue.severity, Severity::Error);
        assert_eq!(issue.path.as_deref(), Some(std::path::Path::new("/Users/me/App/App.xcodeproj")));
        assert_eq!(issue.line, None);
        assert_eq!(
            issue.message,
            "Signing for \"App\" requires a development team. Choose the team in the target’s Signing & Capabilities tab in the project settings."
        );
        assert_eq!(signing_target(&issue.message).as_deref(), Some("App"));
        assert_eq!(signing_target("No profiles for 'a' were found (in target 'Kit' from project 'K')").as_deref(), Some("Kit"));
        assert!(is_signing_issue(&issue));
        let profile = parse_issue("error: No profiles for 'dev.zcode.App' were found: Xcode couldn't find any iOS App Development provisioning profiles matching 'dev.zcode.App'.").unwrap();
        assert!(is_signing_issue(&profile));
        assert!(profile.message.ends_with("Signing & Capabilities tab in the project settings, and sign in to your Apple account in Xcode ▸ Settings ▸ Accounts."), "{}", profile.message);
        assert!(!is_signing_issue(&parse_issue("/a/B.swift:1:2: error: cannot find 'x' in scope").unwrap()));
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
        assert_eq!((t.module.as_deref(), t.full_name().as_str()), (Some("HelloZTests"), "CounterTests/testIncrement"));
        assert_eq!(t.status, TestStatus::Passed);
        let t = parse_test("✘ Test resetClearsCount() failed after 0.002 seconds with 1 issue.")
            .unwrap();
        assert_eq!(t.name, "resetClearsCount()");
        assert_eq!(t.status, TestStatus::Failed);
        let t = parse_test("✔ Test incrementAddsOne() passed after 0.001 seconds.").unwrap();
        assert_eq!(t.status, TestStatus::Passed);
        let t = parse_test(r#"✔ Test "Named test" passed after 0.001 seconds."#).unwrap();
        assert_eq!(t.name, r#""Named test""#);
        let t = parse_test(r#"✔ Test "Adds one" (addsOne()) passed after 0.001 seconds."#).unwrap();
        assert_eq!(t.name, "addsOne()");
        let t = parse_test("➜ Test later() skipped: \"not yet\"").unwrap();
        assert_eq!((t.name.as_str(), t.status), ("later()", TestStatus::Skipped));
    }

    #[test]
    fn parses_xcodebuild_test_names() {
        let t = parse_test("Test case 'GreeterTests.testGreeting()' passed on 'My Mac - HelloClassic (82577)' (0.001 seconds)").unwrap();
        assert_eq!((t.suites.clone(), t.name.as_str(), t.module), (vec!["GreeterTests".to_string()], "testGreeting()", None));
        let t = parse_test("Test case 'Outer/Inner/deep()' failed on 'iPhone 17' (0.000 seconds)").unwrap();
        assert_eq!((t.full_name().as_str(), t.status), ("Outer/Inner/deep()", TestStatus::Failed));
        let t = parse_test("Test case 'globalCheck()' passed on 'My Mac' (0.000 seconds)").unwrap();
        assert!(t.suites.is_empty());
    }

    #[test]
    fn parses_suite_events() {
        assert_eq!(parse_suite_event("◇ Suite CounterTests started."), Some(SuiteEvent::Started("CounterTests".into())));
        assert_eq!(parse_suite_event(r#"✘ Suite "Fancy name" failed after 0.1 seconds with 1 issue."#), Some(SuiteEvent::Finished("Fancy name".into())));
        assert_eq!(parse_suite_event("✔ Test run with 4 tests in 2 suites passed after 0.001 seconds."), None);
        assert_eq!(parse_test("◇ Suite CounterTests started."), None);
        // XCTest's own suite lines are not Swift Testing suites.
        assert_eq!(parse_suite_event("Test Suite 'LegacyTests' started at 2026-10-16 11:51:28.644."), None);
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

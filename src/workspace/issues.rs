//! Issues: live diagnostics from the language server and the last build's
//! errors and warnings, in the Issue navigator and inline in every pane.

use std::path::{Path, PathBuf};

use gpui_kit::*;
use zcode_editor::{Diagnostic as EditorDiagnostic, DiagnosticSeverity, EditorState, Position, RopeExt as _};

use super::{Target, Workspace};
use crate::build::log::{Issue, IssueSource, Severity};
use crate::lsp;

/// A live or build diagnostic of a file.
pub(super) struct FileDiagnostic {
    pub range: lsp_types::Range,
    pub message: String,
    pub severity: DiagnosticSeverity,
    pub source: &'static str,
}

/// Whether two issues are one: the same file and line, the same column
/// when both have one, and the same message but for case, punctuation and
/// a fix-it suffix. sourcekit-lsp's live check and the build report most
/// errors both, worded a little differently ("Consecutive declarations on a
/// line must be separated by ';'" against the build's "consecutive
/// declarations on a line must be separated by ';' (fix-it: insert “;”)");
/// Xcode lists them once.
pub(super) fn same_issue(a: &Issue, b: &Issue) -> bool {
    a.path == b.path
        && a.line == b.line
        && (a.column.is_none() || b.column.is_none() || a.column == b.column)
        && message_key(&a.message) == message_key(&b.message)
}

/// A message's first line without its fix-its (`build::log`'s suffix), in
/// lowercase words with no punctuation between them.
fn message_key(message: &str) -> String {
    let line = message.lines().next().unwrap_or("");
    let line = line.rfind(" (fix-it: ").map_or(line, |at| &line[..at]);
    let words: String = line.chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>().to_lowercase();
    words.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A live diagnostic of `path` as an issue (1-based line and column);
/// `None` for hints and information, which aren't issues.
fn live_issue(path: &Path, d: &lsp_types::Diagnostic) -> Option<Issue> {
    let severity = match d.severity {
        Some(lsp_types::DiagnosticSeverity::ERROR) => Severity::Error,
        Some(lsp_types::DiagnosticSeverity::WARNING) => Severity::Warning,
        _ => return None,
    };
    Some(Issue {
        severity,
        message: d.message.lines().next().unwrap_or("").to_string(),
        path: Some(path.to_path_buf()),
        line: Some(d.range.start.line + 1),
        column: Some(d.range.start.character + 1),
        source: IssueSource::Live,
    })
}

impl Workspace {
    /// Test failures name only the file; map such paths into the project.
    pub(super) fn resolve_issue_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            return path.to_path_buf();
        }
        self.files
            .iter()
            .find(|f| f.file_name() == path.file_name())
            .map(|f| self.project.root.join(f))
            .unwrap_or_else(|| path.to_path_buf())
    }

    /// Combined, de-duplicated issues: live (LSP) diagnostics plus the last
    /// build's issues, a build issue the live check reports too listed once
    /// (as the live one, which follows edits; the build's comes back if the
    /// live one goes before the next build).
    pub fn all_issues(&self, cx: &App) -> Vec<Issue> {
        let mut issues: Vec<Issue> = vec![];
        for (path, diagnostics) in self.lsp.read(cx).diagnostics() {
            issues.extend(diagnostics.iter().filter_map(|d| live_issue(path, d)));
        }
        issues.extend(self.packages.issues.iter().cloned());
        for issue in &self.build.read(cx).issues {
            if issue.severity == Severity::Note {
                continue;
            }
            let mut issue = issue.clone();
            issue.path = issue.path.as_deref().map(|p| self.resolve_issue_path(p));
            if !issues.iter().any(|i| same_issue(i, &issue)) {
                issues.push(issue);
            }
        }
        issues.sort_by(|a, b| {
            a.severity
                .cmp(&b.severity)
                .then((a.source == IssueSource::Package).cmp(&(b.source == IssueSource::Package)).reverse())
                .then(a.path.cmp(&b.path))
                .then(a.line.cmp(&b.line))
        });
        issues
    }

    /// `path`'s live and build diagnostics, in the language server's
    /// coordinates (a build issue covers one character).
    pub(super) fn file_diagnostics(&self, path: &Path, cx: &App) -> Vec<FileDiagnostic> {
        let live = self.lsp.read(cx).diagnostics_for(path);
        let mut entries: Vec<FileDiagnostic> = live
            .iter()
            .map(|d| {
                let severity = match d.severity {
                    Some(lsp_types::DiagnosticSeverity::ERROR) => DiagnosticSeverity::Error,
                    Some(lsp_types::DiagnosticSeverity::WARNING) => DiagnosticSeverity::Warning,
                    Some(lsp_types::DiagnosticSeverity::INFORMATION) => DiagnosticSeverity::Info,
                    _ => DiagnosticSeverity::Hint,
                };
                let mut range = d.range;
                if range.end == range.start {
                    range.end.character += 1;
                }
                FileDiagnostic { range, message: d.message.clone(), severity, source: "sourcekit-lsp" }
            })
            .collect();
        let build_issues = self
            .build
            .read(cx)
            .issues
            .iter()
            .filter(|i| i.path.as_deref().map(|p| self.resolve_issue_path(p)).as_deref() == Some(path));
        for issue in build_issues {
            let line = issue.line.unwrap_or(1).saturating_sub(1);
            let col = issue.column.unwrap_or(1).saturating_sub(1);
            let severity = match issue.severity {
                Severity::Error => DiagnosticSeverity::Error,
                Severity::Warning => DiagnosticSeverity::Warning,
                Severity::Note => DiagnosticSeverity::Info,
            };
            // One mark for an issue the live check has too.
            let mut resolved = issue.clone();
            resolved.path = Some(path.to_path_buf());
            let duplicate = live.iter().filter_map(|d| live_issue(path, d)).any(|d| same_issue(&d, &resolved));
            if !duplicate {
                let start = lsp_types::Position::new(line, col);
                let range = lsp_types::Range::new(start, lsp_types::Position::new(line, col + 1));
                entries.push(FileDiagnostic { range, message: issue.message.clone(), severity, source: "build" });
            }
        }
        entries
    }

    /// Live and build diagnostics, in every pane showing `path`, and in the
    /// multibuffers with excerpts of it.
    pub(super) fn refresh_diagnostics_for(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.refresh_multibuffer_diagnostics_for(path, cx);
        let views: Vec<Entity<EditorState>> = self.views_of(path).iter().map(|item| item.editor.clone()).collect();
        if views.is_empty() {
            return;
        }
        let diagnostics = self.file_diagnostics(path, cx);
        for view in views {
            view.update(cx, |state, cx| {
                let text = state.text().clone();
                let to_pos = |p: lsp_types::Position| -> Position {
                    text.offset_to_position(lsp::lsp_to_offset(&text, p))
                };
                let entries: Vec<EditorDiagnostic> = diagnostics
                    .iter()
                    .map(|d| {
                        EditorDiagnostic::new(to_pos(d.range.start)..to_pos(d.range.end), d.message.clone())
                            .with_severity(d.severity)
                            .with_source(d.source)
                    })
                    .collect();
                if let Some(set) = state.diagnostics_mut() {
                    set.clear();
                    set.extend(entries);
                }
                cx.notify();
            });
        }
    }

    pub fn open_issue(&mut self, issue: &Issue, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = issue.path.clone() else { return };
        // Project-level issues (signing) are fixed in the project settings,
        // signing errors in the target's Signing & Capabilities tab.
        if path.extension().is_some_and(|e| e == "xcodeproj" || e == "xcworkspace") {
            self.open_project_settings(window, cx);
            if crate::build::log::is_signing_issue(issue)
                && let Some(settings) = self.project_settings.clone()
            {
                let target = crate::build::log::signing_target(&issue.message).unwrap_or_default();
                settings.update(cx, |settings, cx| {
                    if settings.show(&target, "Signing", window, cx).is_err() {
                        settings.show_signing(window, cx);
                    }
                });
            }
            return;
        }
        let target = Target::Line {
            line: issue.line.unwrap_or(1).saturating_sub(1),
            column: issue.column.unwrap_or(1).saturating_sub(1),
        };
        self.open_file(path, Some(target), true, window, cx);
    }

    pub(super) fn step_issue(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let issues: Vec<Issue> = self
            .all_issues(cx)
            .into_iter()
            .filter(|i| i.path.is_some())
            .collect();
        if issues.is_empty() {
            return;
        }
        let len = issues.len() as isize;
        let next = match self.issue_cursor {
            Some(ix) => ((ix as isize + delta).rem_euclid(len)) as usize,
            None if delta >= 0 => 0,
            None => (len - 1) as usize,
        };
        self.issue_cursor = Some(next);
        let issue = issues[next].clone();
        self.open_issue(&issue, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{Issue, IssueSource, Severity, same_issue};

    fn issue(source: IssueSource, line: u32, column: Option<u32>, message: &str) -> Issue {
        Issue { severity: Severity::Error, message: message.into(), path: Some("/p/Greeter.swift".into()), line: Some(line), column, source }
    }

    #[test]
    fn a_build_issue_matches_the_live_one_worded_differently() {
        let live = issue(IssueSource::Live, 3, Some(28), "Consecutive declarations on a line must be separated by ';'");
        let build = issue(IssueSource::Build, 3, Some(28), "consecutive declarations on a line must be separated by ';' (fix-it: insert “;”)");
        assert!(same_issue(&live, &build) && same_issue(&build, &live));
        // A build issue without a column matches on the line.
        assert!(same_issue(&live, &issue(IssueSource::Build, 3, None, "consecutive declarations on a line must be separated by ';'")));
        // Only the first line of a message counts, and spacing doesn't.
        assert!(same_issue(&live, &issue(IssueSource::Build, 3, Some(28), "Consecutive  declarations on a line must be separated by ';'.\nnote: here")));
        let use_of = issue(IssueSource::Live, 7, Some(5), "Cannot find 'oops' in scope");
        assert!(same_issue(&use_of, &issue(IssueSource::Build, 7, Some(5), "cannot find 'oops' in scope")));
    }

    #[test]
    fn issues_elsewhere_or_saying_something_else_stay_apart() {
        let live = issue(IssueSource::Live, 3, Some(28), "Consecutive declarations on a line must be separated by ';'");
        assert!(!same_issue(&live, &issue(IssueSource::Build, 4, Some(28), "consecutive declarations on a line must be separated by ';'")));
        assert!(!same_issue(&live, &issue(IssueSource::Build, 3, Some(9), "consecutive declarations on a line must be separated by ';'")));
        assert!(!same_issue(&live, &issue(IssueSource::Build, 3, Some(28), "expected declaration")));
        let mut other_file = issue(IssueSource::Build, 3, Some(28), "consecutive declarations on a line must be separated by ';'");
        other_file.path = Some("/p/Other.swift".into());
        assert!(!same_issue(&live, &other_file));
        // Words matter, not just letters: 'a b' isn't 'ab'.
        assert!(!same_issue(&issue(IssueSource::Live, 1, None, "use of 'a b'"), &issue(IssueSource::Build, 1, None, "use of 'ab'")));
        // The fix-it suffix is the build's; text in parentheses otherwise counts.
        assert!(!same_issue(&issue(IssueSource::Live, 1, None, "unused (x)"), &issue(IssueSource::Build, 1, None, "unused")));
    }
}

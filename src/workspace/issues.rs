//! Issues: live diagnostics from the language server and the last build's
//! errors and warnings, in the Issue navigator and inline in every pane.

use std::path::{Path, PathBuf};

use gpui_kit::*;
use zcode_editor::{Diagnostic as EditorDiagnostic, DiagnosticSeverity, EditorState, Position, RopeExt as _};

use super::{Target, Workspace};
use crate::build::log::{Issue, IssueSource, Severity};
use crate::lsp;

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
    /// build's issues.
    pub fn all_issues(&self, cx: &App) -> Vec<Issue> {
        let mut issues: Vec<Issue> = vec![];
        for (path, diagnostics) in self.lsp.read(cx).diagnostics() {
            for d in diagnostics {
                let severity = match d.severity {
                    Some(lsp_types::DiagnosticSeverity::ERROR) => Severity::Error,
                    Some(lsp_types::DiagnosticSeverity::WARNING) => Severity::Warning,
                    _ => continue,
                };
                issues.push(Issue {
                    severity,
                    message: d.message.lines().next().unwrap_or("").to_string(),
                    path: Some(path.clone()),
                    line: Some(d.range.start.line + 1),
                    column: Some(d.range.start.character + 1),
                    source: IssueSource::Live,
                });
            }
        }
        issues.extend(self.packages.issues.iter().cloned());
        for issue in &self.build.read(cx).issues {
            if issue.severity == Severity::Note {
                continue;
            }
            let mut issue = issue.clone();
            issue.path = issue.path.as_deref().map(|p| self.resolve_issue_path(p));
            let issue = &issue;
            let duplicate = issues.iter().any(|i| {
                i.path == issue.path && i.line == issue.line && i.message == issue.message
            });
            if !duplicate {
                issues.push(issue.clone());
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

    /// Live and build diagnostics, in every pane showing `path`.
    pub(super) fn refresh_diagnostics_for(&mut self, path: &Path, cx: &mut Context<Self>) {
        let views: Vec<Entity<EditorState>> = self.views_of(path).iter().map(|item| item.editor.clone()).collect();
        if views.is_empty() {
            return;
        }
        let live = self.lsp.read(cx).diagnostics_for(path).to_vec();
        let build_issues: Vec<Issue> = self
            .build
            .read(cx)
            .issues
            .iter()
            .filter(|i| i.path.as_deref().map(|p| self.resolve_issue_path(p)).as_deref() == Some(path))
            .cloned()
            .collect();
        for view in views {
            view.update(cx, |state, cx| {
                let text = state.text().clone();
                let to_pos = |p: lsp_types::Position| -> Position {
                    text.offset_to_position(lsp::lsp_to_offset(&text, p))
                };
                let mut entries: Vec<EditorDiagnostic> = live
                    .iter()
                    .map(|d| {
                        let severity = match d.severity {
                            Some(lsp_types::DiagnosticSeverity::ERROR) => DiagnosticSeverity::Error,
                            Some(lsp_types::DiagnosticSeverity::WARNING) => DiagnosticSeverity::Warning,
                            Some(lsp_types::DiagnosticSeverity::INFORMATION) => DiagnosticSeverity::Info,
                            _ => DiagnosticSeverity::Hint,
                        };
                        let mut end = d.range.end;
                        if end == d.range.start {
                            end.character += 1;
                        }
                        EditorDiagnostic::new(to_pos(d.range.start)..to_pos(end), d.message.clone())
                            .with_severity(severity)
                            .with_source("sourcekit-lsp")
                    })
                    .collect();
                for issue in &build_issues {
                    let line = issue.line.unwrap_or(1).saturating_sub(1);
                    let col = issue.column.unwrap_or(1).saturating_sub(1);
                    let start = to_pos(lsp_types::Position::new(line, col));
                    let end = Position::new(start.line, start.character + 1);
                    let severity = match issue.severity {
                        Severity::Error => DiagnosticSeverity::Error,
                        Severity::Warning => DiagnosticSeverity::Warning,
                        Severity::Note => DiagnosticSeverity::Info,
                    };
                    let duplicate = live.iter().any(|d| d.range.start.line == line && d.message == issue.message);
                    if !duplicate {
                        entries.push(
                            EditorDiagnostic::new(start..end, issue.message.clone())
                                .with_severity(severity)
                                .with_source("build"),
                        );
                    }
                }
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

//! @-mentions in the agent panel's message field: a picker of the
//! project's files and, from the language server, its symbols (as Open
//! Quickly lists them). A mention is an inline token whose id is the file's
//! URI, with `#L<first>:<last>` (1-based) for a symbol's lines; sending
//! the message embeds the file's text, from its buffer, for the agent.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::*;


use super::AgentPanel;
use crate::fuzzy::fuzzy_match;
use crate::lsp::{self, LanguageServer};
use crate::picker::{Picker, PickerDelegate, PickerItem};
use crate::workspace::Workspace;
use crate::workspace::pickers::{file_icon, symbol_icon, symbol_kind_name};

const MAX_RESULTS: usize = 60;

enum Mention {
    File(PathBuf),
    Symbol { path: PathBuf, name: String, lines: Range<u32> },
}

pub struct MentionDelegate {
    workspace: WeakEntity<Workspace>,
    panel: WeakEntity<AgentPanel>,
    /// Where the "@" that opened the picker is in the message.
    at: Option<usize>,
    root: PathBuf,
    files: Arc<Vec<PathBuf>>,
    labels: Vec<String>,
    servers: Vec<Arc<LanguageServer>>,
    items: Vec<PickerItem>,
    matches: Vec<Mention>,
    symbol_task: Option<Task<()>>,
}

impl MentionDelegate {
    pub fn new(workspace: WeakEntity<Workspace>, panel: WeakEntity<AgentPanel>, at: Option<usize>, root: PathBuf, files: Arc<Vec<PathBuf>>, servers: Vec<Arc<LanguageServer>>) -> Self {
        let labels = files.iter().map(|p| p.to_string_lossy().to_string()).collect();
        Self { workspace, panel, at, root, files, labels, servers, items: vec![], matches: vec![], symbol_task: None }
    }
}

impl PickerDelegate for MentionDelegate {
    fn placeholder(&self) -> SharedString {
        "Mention a File or Symbol".into()
    }

    fn two_line(&self) -> bool {
        true
    }

    fn items(&self) -> &[PickerItem] {
        &self.items
    }

    fn update_matches(&mut self, query: String, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        let query = query.trim().to_string();
        self.items.clear();
        self.matches.clear();
        for m in fuzzy_match(&query, &self.labels, true, MAX_RESULTS) {
            let rel = &self.files[m.index];
            let name = rel.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let dir = rel.parent().map(|p| p.display().to_string()).unwrap_or_default();
            self.items.push(PickerItem {
                label: name.into(),
                detail: Some(if dir.is_empty() { "/".into() } else { dir.into() }),
                icon: Some(file_icon(rel)),
                ..Default::default()
            });
            self.matches.push(Mention::File(self.root.join(rel)));
        }
        // Symbols arrive from the language server below the files.
        self.symbol_task = None;
        if query.chars().count() >= 2
            && !self.servers.is_empty()
        {
            let request = lsp::workspace_symbols(&self.servers, &query);
            let root = self.root.clone();
            self.symbol_task = Some(cx.spawn(async move |picker, cx| {
                let Ok(value) = request.await else { return };
                let symbols: Vec<lsp_types::SymbolInformation> = serde_json::from_value(value).unwrap_or_default();
                picker
                    .update(cx, |picker, cx| {
                        let delegate = &mut picker.delegate;
                        for symbol in symbols.into_iter().take(MAX_RESULTS) {
                            let Some(path) = lsp::uri_to_path(&symbol.location.uri) else { continue };
                            if !path.starts_with(&root) {
                                continue;
                            }
                            let kind = serde_json::to_value(symbol.kind).ok().and_then(|v| v.as_u64()).unwrap_or(0);
                            let range = symbol.location.range;
                            delegate.items.push(PickerItem {
                                label: symbol.name.clone().into(),
                                detail: Some(path.strip_prefix(&root).unwrap_or(&path).display().to_string().into()),
                                icon: Some(symbol_icon(kind)),
                                hint: Some(symbol_kind_name(kind).into()),
                                ..Default::default()
                            });
                            delegate.matches.push(Mention::Symbol { path, name: symbol.name, lines: range.start.line..range.end.line });
                        }
                        picker.matches_updated(cx);
                    })
                    .ok();
            }));
        }
    }

    fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        let Some(mention) = self.matches.get(index) else { return true };
        let (uri, label) = match mention {
            Mention::File(path) => (uri(path, None), label(path, None)),
            Mention::Symbol { path, name, lines } => (uri(path, Some(lines.clone())), name.clone()),
        };
        let at = self.at;
        self.workspace.update(cx, |ws, cx| ws.dismiss_modal(window, cx)).ok();
        self.panel.update(cx, |panel, cx| panel.insert_mention(uri, label, at, window, cx)).ok();
        false
    }
}

/// A mention's id: the file's URI, with its lines (0-based, inclusive,
/// written 1-based) for a symbol.
pub fn uri(path: &Path, lines: Option<Range<u32>>) -> String {
    let base = format!("file://{}", path.display());
    match lines {
        Some(lines) => format!("{base}#L{}:{}", lines.start + 1, lines.end + 1),
        None => base,
    }
}

pub fn parse_uri(uri: &str) -> (PathBuf, Option<Range<u32>>) {
    let rest = uri.strip_prefix("file://").unwrap_or(uri);
    match rest.rsplit_once("#L") {
        Some((path, lines)) => {
            let range = lines.split_once(':').and_then(|(a, b)| Some(a.parse::<u32>().ok()?.saturating_sub(1)..b.parse::<u32>().ok()?.saturating_sub(1)));
            (PathBuf::from(path), range)
        }
        None => (PathBuf::from(rest), None),
    }
}

/// What a mention shows: the file's name, and the lines for a range.
pub fn label(path: &Path, lines: Option<&Range<u32>>) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    match lines {
        Some(lines) => format!("{name}:{}-{}", lines.start + 1, lines.end + 1),
        None => name,
    }
}

#[cfg(test)]
mod tests {
    use super::{label, parse_uri, uri};
    use std::path::Path;

    #[test]
    fn uris_round_trip() {
        let path = Path::new("/p/Sources/A B.swift");
        assert_eq!(uri(path, None), "file:///p/Sources/A B.swift");
        assert_eq!(parse_uri(&uri(path, None)), (path.to_path_buf(), None));
        assert_eq!(uri(path, Some(4..9)), "file:///p/Sources/A B.swift#L5:10");
        assert_eq!(parse_uri(&uri(path, Some(4..9))), (path.to_path_buf(), Some(4..9)));
        assert_eq!(label(path, Some(&(4..9))), "A B.swift:5-10");
    }
}

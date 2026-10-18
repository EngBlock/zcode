//! Picker delegates used by the workspace.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::*;
use serde_json::Value;

use super::{Target, Workspace};
use crate::build::BuildController;
use crate::build::xcode::{Destination, Platform};
use crate::fuzzy::fuzzy_match;
use crate::lsp::{self, LanguageServer};
use crate::picker::{Picker, PickerDelegate, PickerItem};

const MAX_RESULTS: usize = 200;

pub fn symbol_icon(kind: u64) -> IconName {
    match kind {
        5 | 23 => IconName::Box,         // class, struct
        10 | 22 => IconName::ListOrdered, // enum, enum member
        11 => IconName::Shapes,          // interface/protocol
        6 | 9 | 12 => IconName::SquareFunction,
        7 | 8 | 13 | 14 => IconName::Variable,
        2..=4 => IconName::Package,
        _ => IconName::Hash,
    }
}

pub fn symbol_kind_name(kind: u64) -> &'static str {
    match kind {
        1 => "file",
        2 => "module",
        3 => "namespace",
        4 => "package",
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "init",
        10 => "enum",
        11 => "protocol",
        12 => "func",
        13 => "var",
        14 => "let",
        22 => "case",
        23 => "struct",
        26 => "type parameter",
        _ => "",
    }
}

// ---------------------------------------------------------------------------
// Open Quickly (⇧⌘O): files plus workspace symbols.

enum OpenQuicklyMatch {
    File { path: PathBuf, line: Option<u32> },
    Symbol { path: PathBuf, range: lsp_types::Range },
}

pub struct OpenQuicklyDelegate {
    workspace: WeakEntity<Workspace>,
    root: PathBuf,
    files: Arc<Vec<PathBuf>>,
    labels: Arc<Vec<String>>,
    servers: Vec<Arc<LanguageServer>>,
    items: Vec<PickerItem>,
    matches: Vec<OpenQuicklyMatch>,
    symbol_task: Option<Task<()>>,
}

impl OpenQuicklyDelegate {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        root: PathBuf,
        files: Arc<Vec<PathBuf>>,
        servers: Vec<Arc<LanguageServer>>,
    ) -> Self {
        let labels = Arc::new(files.iter().map(|p| p.to_string_lossy().to_string()).collect());
        Self {
            workspace,
            root,
            files,
            labels,
            servers,
            items: vec![],
            matches: vec![],
            symbol_task: None,
        }
    }
}

fn split_line_suffix(query: &str) -> (&str, Option<u32>) {
    if let Some((head, tail)) = query.rsplit_once(':')
        && let Ok(line) = tail.trim().parse::<u32>()
    {
        return (head, Some(line));
    }
    (query, None)
}

impl PickerDelegate for OpenQuicklyDelegate {
    fn placeholder(&self) -> SharedString {
        "Open Quickly".into()
    }

    fn two_line(&self) -> bool {
        true
    }

    fn items(&self) -> &[PickerItem] {
        &self.items
    }

    fn update_matches(&mut self, query: String, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        let (file_query, line) = split_line_suffix(&query);
        let file_query = file_query.trim().to_string();
        let started = std::time::Instant::now();
        let found = fuzzy_match(&file_query, &self.labels, true, MAX_RESULTS);
        crate::perf::Perf::record_timing(cx, "open_quickly", started.elapsed(), self.labels.len());
        self.items.clear();
        self.matches.clear();
        for m in found {
            let rel = &self.files[m.index];
            let label = &self.labels[m.index];
            let name = rel
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let name_start = label.chars().count() - name.chars().count();
            let positions = m
                .positions
                .iter()
                .filter(|&&p| p >= name_start)
                .map(|p| p - name_start)
                .collect();
            let dir = rel
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            self.items.push(PickerItem {
                label: name.into(),
                detail: Some(if dir.is_empty() { "/".into() } else { dir.into() }),
                icon: Some(file_icon(rel)),
                positions,
                hint: line.map(|l| format!("line {l}").into()),
                ..Default::default()
            });
            self.matches.push(OpenQuicklyMatch::File {
                path: self.root.join(rel),
                line,
            });
        }

        // Workspace symbols arrive asynchronously, below the files.
        self.symbol_task = None;
        if file_query.chars().count() >= 2
            && !self.servers.is_empty()
        {
            let request = lsp::workspace_symbols(&self.servers, &file_query);
            let root = self.root.clone();
            self.symbol_task = Some(cx.spawn(async move |picker, cx| {
                let Ok(value) = request.await else { return };
                let symbols: Vec<lsp_types::SymbolInformation> =
                    serde_json::from_value(value).unwrap_or_default();
                picker
                    .update(cx, |picker, cx| {
                        let delegate = &mut picker.delegate;
                        for symbol in symbols.into_iter().take(60) {
                            let Some(path) = lsp::uri_to_path(&symbol.location.uri) else {
                                continue;
                            };
                            // Keep project symbols; SDK symbols are noise here.
                            if !path.starts_with(&root) {
                                continue;
                            }
                            let kind = serde_json::to_value(symbol.kind)
                                .ok()
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            delegate.items.push(PickerItem {
                                label: symbol.name.clone().into(),
                                detail: Some(
                                    format!(
                                        "{} — {}",
                                        symbol.container_name.clone().unwrap_or_default(),
                                        path.strip_prefix(&root).unwrap_or(&path).display()
                                    )
                                    .trim_start_matches(" — ")
                                    .to_string()
                                    .into(),
                                ),
                                icon: Some(symbol_icon(kind)),
                                hint: Some(symbol_kind_name(kind).into()),
                                ..Default::default()
                            });
                            delegate.matches.push(OpenQuicklyMatch::Symbol {
                                path,
                                range: symbol.location.range,
                            });
                        }
                        picker.matches_updated(cx);
                    })
                    .ok();
            }));
        }
    }

    fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        self.open(index, false, window, cx)
    }

    /// ⌥-Return opens in the next pane, the assistant when there is one.
    fn confirm_alternate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        self.open(index, true, window, cx)
    }
}

impl OpenQuicklyDelegate {
    fn open(&mut self, index: usize, next_pane: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        let (path, target) = match &self.matches[index] {
            OpenQuicklyMatch::File { path, line } => (
                path.clone(),
                line.map(|l| Target::Line {
                    line: l.saturating_sub(1),
                    column: 0,
                }),
            ),
            OpenQuicklyMatch::Symbol { path, range } => (path.clone(), Some(Target::LspRange(*range))),
        };
        self.workspace
            .update(cx, |ws, cx| {
                ws.dismiss_modal(window, cx);
                if next_pane {
                    ws.open_in_next_pane(path, target, window, cx);
                } else {
                    ws.open_file(path, target, true, window, cx);
                }
            })
            .ok();
        false
    }
}

pub fn file_icon(path: &Path) -> IconName {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "swift" | "m" | "mm" | "h" | "c" | "cpp" | "rs" => IconName::FileCode,
        "json" | "plist" | "yml" | "yaml" | "toml" | "xcconfig" | "entitlements" => IconName::FileCog,
        "md" | "txt" => IconName::FileText,
        "png" | "jpg" | "jpeg" | "pdf" | "svg" => IconName::FileImage,
        "storyboard" | "xib" => IconName::LayoutTemplate,
        "xcassets" => IconName::Images,
        _ => IconName::File,
    }
}

// ---------------------------------------------------------------------------
// Quick Actions (⇧⌘A): every command with its shortcut.

/// (menu, name, action, keystroke label)
type QuickAction = (String, String, Box<dyn Action>, Option<String>);

pub struct QuickActionsDelegate {
    commands: Vec<QuickAction>,
    labels: Vec<String>,
    items: Vec<PickerItem>,
    matches: Vec<usize>,
}

impl QuickActionsDelegate {
    pub fn new(window: &Window) -> Self {
        let commands: Vec<_> = crate::actions::commands()
            .into_iter()
            .filter(|c| c.name != "-")
            .map(|c| {
                let action = (c.action)();
                let keys = window
                    .highest_precedence_binding_for_action(action.as_ref())
                    .map(|b| {
                        b.keystrokes()
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(" ")
                    });
                (c.menu.to_string(), c.name.to_string(), action, keys)
            })
            .collect();
        let labels = commands
            .iter()
            .map(|(menu, name, ..)| format!("{name} {menu}"))
            .collect();
        Self {
            commands,
            labels,
            items: vec![],
            matches: vec![],
        }
    }
}

impl PickerDelegate for QuickActionsDelegate {
    fn placeholder(&self) -> SharedString {
        "Quick Actions".into()
    }

    fn items(&self) -> &[PickerItem] {
        &self.items
    }

    fn update_matches(&mut self, query: String, _: &mut Window, _: &mut Context<Picker<Self>>) {
        let found = fuzzy_match(&query, &self.labels, false, MAX_RESULTS);
        self.matches = found.iter().map(|m| m.index).collect();
        self.items = found
            .into_iter()
            .map(|m| {
                let (menu, name, _, keys) = &self.commands[m.index];
                let name_len = name.chars().count();
                PickerItem {
                    label: name.clone().into(),
                    detail: Some(menu.clone().into()),
                    positions: m.positions.into_iter().filter(|&p| p < name_len).collect(),
                    hint: keys.clone().map(Into::into),
                    ..Default::default()
                }
            })
            .collect();
    }

    fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        let action = self.commands[self.matches[index]].2.boxed_clone();
        // Run after the palette closes and focus returns to the editor.
        window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
        true
    }
}

// ---------------------------------------------------------------------------
// Document Items (⌃6) and the Symbol navigator.

#[derive(Clone, Debug)]
pub struct OutlineEntry {
    pub name: String,
    pub detail: Option<String>,
    pub kind: u64,
    pub depth: usize,
    pub selection_range: lsp_types::Range,
}

/// Flattens `textDocument/documentSymbol` (hierarchical or flat) into rows.
pub fn flatten_symbols(value: Value) -> Vec<OutlineEntry> {
    fn walk(symbols: &[Value], depth: usize, out: &mut Vec<OutlineEntry>) {
        for symbol in symbols {
            let range = symbol
                .get("selectionRange")
                .or_else(|| symbol.pointer("/location/range"))
                .cloned()
                .and_then(|r| serde_json::from_value(r).ok())
                .unwrap_or_default();
            out.push(OutlineEntry {
                name: symbol
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                detail: symbol
                    .get("detail")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                kind: symbol.get("kind").and_then(Value::as_u64).unwrap_or(0),
                depth,
                selection_range: range,
            });
            if let Some(children) = symbol.get("children").and_then(Value::as_array) {
                walk(children, depth + 1, out);
            }
        }
    }
    let mut out = vec![];
    if let Some(symbols) = value.as_array() {
        walk(symbols, 0, &mut out);
    }
    out
}

pub struct DocumentItemsDelegate {
    workspace: WeakEntity<Workspace>,
    entries: Vec<OutlineEntry>,
    labels: Vec<String>,
    items: Vec<PickerItem>,
    matches: Vec<usize>,
    initial: usize,
}

impl DocumentItemsDelegate {
    pub fn new(workspace: WeakEntity<Workspace>, entries: Vec<OutlineEntry>, cursor_line: u32) -> Self {
        let labels = entries.iter().map(|e| e.name.clone()).collect();
        let initial = entries
            .iter()
            .rposition(|e| e.selection_range.start.line <= cursor_line)
            .unwrap_or(0);
        Self {
            workspace,
            entries,
            labels,
            items: vec![],
            matches: vec![],
            initial,
        }
    }
}

impl DocumentItemsDelegate {
    pub fn set_entries(&mut self, entries: Vec<OutlineEntry>, cursor_line: u32) {
        self.labels = entries.iter().map(|e| e.name.clone()).collect();
        self.initial = entries
            .iter()
            .rposition(|e| e.selection_range.start.line <= cursor_line)
            .unwrap_or(0);
        self.entries = entries;
    }
}

impl PickerDelegate for DocumentItemsDelegate {
    fn placeholder(&self) -> SharedString {
        "Document Items".into()
    }

    fn items(&self) -> &[PickerItem] {
        &self.items
    }

    fn initial_selection(&self) -> usize {
        self.initial
    }

    fn empty_message(&self) -> SharedString {
        "No symbols (is the language server ready?)".into()
    }

    fn update_matches(&mut self, query: String, _: &mut Window, _: &mut Context<Picker<Self>>) {
        let filtering = !query.trim().is_empty();
        let found = fuzzy_match(&query, &self.labels, false, 1000);
        let mut indices: Vec<_> = found.into_iter().collect();
        if !filtering {
            indices.sort_by_key(|m| m.index);
        }
        self.matches = indices.iter().map(|m| m.index).collect();
        self.items = indices
            .into_iter()
            .map(|m| {
                let entry = &self.entries[m.index];
                PickerItem {
                    label: entry.name.clone().into(),
                    icon: Some(symbol_icon(entry.kind)),
                    positions: m.positions,
                    hint: Some(symbol_kind_name(entry.kind).into()),
                    depth: if filtering { 0 } else { entry.depth },
                    ..Default::default()
                }
            })
            .collect();
    }

    fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        let entry = self.entries[self.matches[index]].clone();
        self.workspace
            .update(cx, |ws, cx| {
                ws.dismiss_modal(window, cx);
                ws.reveal_outline_entry(&entry, window, cx);
            })
            .ok();
        false
    }
}

// ---------------------------------------------------------------------------
// Jump to Line (⌘L).

pub struct JumpToLineDelegate {
    workspace: WeakEntity<Workspace>,
    line_count: usize,
    target: Option<(u32, u32)>,
    items: Vec<PickerItem>,
}

impl JumpToLineDelegate {
    pub fn new(workspace: WeakEntity<Workspace>, line_count: usize) -> Self {
        Self {
            workspace,
            line_count,
            target: None,
            items: vec![],
        }
    }
}

impl PickerDelegate for JumpToLineDelegate {
    fn placeholder(&self) -> SharedString {
        format!("Line number (1–{}), optionally line:column", self.line_count).into()
    }

    fn items(&self) -> &[PickerItem] {
        &self.items
    }

    fn empty_message(&self) -> SharedString {
        "Type a line number".into()
    }

    fn update_matches(&mut self, query: String, _: &mut Window, _: &mut Context<Picker<Self>>) {
        let mut parts = query.trim().split(':');
        let line = parts.next().and_then(|l| l.trim().parse::<u32>().ok());
        let column = parts.next().and_then(|c| c.trim().parse::<u32>().ok()).unwrap_or(1);
        self.target = line.map(|l| (l.max(1), column.max(1)));
        self.items = match self.target {
            Some((line, column)) => vec![PickerItem {
                label: format!("Jump to line {line}, column {column}").into(),
                icon: Some(IconName::CornerDownRight),
                ..Default::default()
            }],
            None => vec![],
        };
    }

    fn confirm(&mut self, _: usize, window: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        let Some((line, column)) = self.target else { return false };
        self.workspace
            .update(cx, |ws, cx| {
                ws.dismiss_modal(window, cx);
                if let Some(path) = ws.active_item().map(|b| b.path.clone()) {
                    ws.open_file(
                        path,
                        Some(Target::Line {
                            line: line - 1,
                            column: column - 1,
                        }),
                        true,
                        window,
                        cx,
                    );
                }
            })
            .ok();
        false
    }
}

// ---------------------------------------------------------------------------
// Scheme (⌃0) and destination (⌃⇧0) choosers.

pub struct SchemeDelegate {
    build: Entity<BuildController>,
    schemes: Vec<String>,
    current: Option<String>,
    items: Vec<PickerItem>,
    matches: Vec<usize>,
}

impl SchemeDelegate {
    pub fn new(build: Entity<BuildController>, cx: &App) -> Self {
        let state = build.read(cx);
        Self {
            schemes: state.schemes.schemes.clone(),
            current: state.scheme.clone(),
            build,
            items: vec![],
            matches: vec![],
        }
    }
}

impl PickerDelegate for SchemeDelegate {
    fn placeholder(&self) -> SharedString {
        "Choose Scheme".into()
    }

    fn items(&self) -> &[PickerItem] {
        &self.items
    }

    fn empty_message(&self) -> SharedString {
        "No schemes".into()
    }

    fn update_matches(&mut self, query: String, _: &mut Window, _: &mut Context<Picker<Self>>) {
        let found = fuzzy_match(&query, &self.schemes, false, MAX_RESULTS);
        self.matches = found.iter().map(|m| m.index).collect();
        self.items = found
            .into_iter()
            .map(|m| PickerItem {
                label: self.schemes[m.index].clone().into(),
                icon: Some(IconName::Hammer),
                positions: m.positions,
                checked: self.current.as_ref() == Some(&self.schemes[m.index]),
                ..Default::default()
            })
            .collect();
    }

    fn confirm(&mut self, index: usize, _: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        let scheme = self.schemes[self.matches[index]].clone();
        self.build.update(cx, |build, cx| build.set_scheme(scheme, cx));
        true
    }
}

pub fn destination_icon(destination: &Destination) -> IconName {
    match destination.platform {
        Platform::MacOS => IconName::Laptop,
        Platform::IOSSimulator | Platform::IOS if is_ipad(destination) => IconName::Tablet,
        Platform::IOSSimulator | Platform::IOS => IconName::Smartphone,
        Platform::WatchOSSimulator | Platform::WatchOS => IconName::Watch,
        Platform::TvOSSimulator | Platform::TvOS => IconName::Tv,
        Platform::VisionOSSimulator | Platform::VisionOS => IconName::Glasses,
    }
}

fn is_ipad(destination: &Destination) -> bool {
    destination.name.contains("iPad") || destination.device.as_ref().is_some_and(|d| d.model.contains("iPad"))
}

pub struct DestinationDelegate {
    build: Entity<BuildController>,
    destinations: Vec<Destination>,
    labels: Vec<String>,
    current: Option<String>,
    items: Vec<PickerItem>,
    matches: Vec<usize>,
}

impl DestinationDelegate {
    pub fn new(build: Entity<BuildController>, cx: &App) -> Self {
        let state = build.read(cx);
        let destinations = state.destinations.clone();
        let labels = destinations
            .iter()
            .map(|d| format!("{} {}", d.name, d.runtime))
            .collect();
        Self {
            current: state.destination.as_ref().map(|d| d.id.clone()),
            build,
            destinations,
            labels,
            items: vec![],
            matches: vec![],
        }
    }
}

impl PickerDelegate for DestinationDelegate {
    fn placeholder(&self) -> SharedString {
        "Choose Run Destination".into()
    }

    fn items(&self) -> &[PickerItem] {
        &self.items
    }

    fn initial_selection(&self) -> usize {
        self.items.iter().position(|i| i.checked).unwrap_or(0)
    }

    fn update_matches(&mut self, query: String, _: &mut Window, _: &mut Context<Picker<Self>>) {
        let found = fuzzy_match(&query, &self.labels, false, MAX_RESULTS);
        self.matches = found.iter().map(|m| m.index).collect();
        self.items = found
            .into_iter()
            .map(|m| {
                let d = &self.destinations[m.index];
                let name_len = d.name.chars().count();
                PickerItem {
                    label: d.name.clone().into(),
                    detail: Some(d.detail().into()),
                    icon: Some(destination_icon(d)),
                    positions: m.positions.into_iter().filter(|&p| p < name_len).collect(),
                    checked: self.current.as_ref() == Some(&d.id),
                    ..Default::default()
                }
            })
            .collect();
    }

    fn confirm(&mut self, index: usize, _: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        let destination = self.destinations[self.matches[index]].clone();
        self.build
            .update(cx, |build, cx| build.set_destination(destination, cx));
        true
    }
}

// ---------------------------------------------------------------------------
// Rename (F2).

pub struct RenameDelegate {
    workspace: WeakEntity<Workspace>,
    server: Arc<LanguageServer>,
    path: PathBuf,
    position: lsp_types::Position,
    original: String,
    new_name: String,
    items: Vec<PickerItem>,
}

impl RenameDelegate {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        server: Arc<LanguageServer>,
        path: PathBuf,
        position: lsp_types::Position,
        original: String,
    ) -> Self {
        Self {
            workspace,
            server,
            path,
            position,
            new_name: original.clone(),
            original,
            items: vec![],
        }
    }
}

impl PickerDelegate for RenameDelegate {
    fn placeholder(&self) -> SharedString {
        "New name".into()
    }

    fn items(&self) -> &[PickerItem] {
        &self.items
    }

    fn update_matches(&mut self, query: String, _: &mut Window, _: &mut Context<Picker<Self>>) {
        self.new_name = query.trim().to_string();
        let valid = !self.new_name.is_empty()
            && self.new_name != self.original
            && self
                .new_name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_');
        self.items = if valid {
            vec![PickerItem {
                label: format!("Rename “{}” to “{}”", self.original, self.new_name).into(),
                detail: Some("Updates every reference in the workspace".into()),
                icon: Some(IconName::Pencil),
                ..Default::default()
            }]
        } else {
            vec![]
        };
    }

    fn empty_message(&self) -> SharedString {
        format!("Type a new name for “{}”", self.original).into()
    }

    fn confirm(&mut self, _: usize, window: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        let (server, path, position, original, name) = (
            self.server.clone(),
            self.path.clone(),
            self.position,
            self.original.clone(),
            self.new_name.clone(),
        );
        self.workspace
            .update(cx, |ws, cx| {
                ws.dismiss_modal(window, cx);
                ws.rename_symbol(server, path, position, original, name, window, cx);
            })
            .ok();
        false
    }
}

// ---------------------------------------------------------------------------
// Emmet ▸ Wrap with Abbreviation.

pub struct WrapDelegate {
    workspace: WeakEntity<Workspace>,
    editor: Entity<zcode_editor::EditorState>,
    range: std::ops::Range<usize>,
    content: String,
    syntax: zcode_emmet::Syntax,
    options: zcode_emmet::Options,
    indent: String,
    snippet: Option<String>,
    items: Vec<PickerItem>,
}

impl WrapDelegate {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        editor: Entity<zcode_editor::EditorState>,
        range: std::ops::Range<usize>,
        text: zcode_editor::Rope,
        syntax: zcode_emmet::Syntax,
        options: zcode_emmet::Options,
        indent: String,
    ) -> Self {
        let content = text.slice(range.clone()).to_string();
        Self { workspace, editor, range, content, syntax, options, indent, snippet: None, items: vec![] }
    }
}

impl PickerDelegate for WrapDelegate {
    fn placeholder(&self) -> SharedString {
        "Abbreviation (ul>li*, div.card)".into()
    }

    fn items(&self) -> &[PickerItem] {
        &self.items
    }

    fn update_matches(&mut self, query: String, _: &mut Window, _: &mut Context<Picker<Self>>) {
        let abbreviation = query.trim();
        self.snippet = (!abbreviation.is_empty())
            .then(|| crate::languages::emmet::wrap(abbreviation, &self.content, self.syntax, &self.options, &self.indent))
            .flatten();
        self.items = match &self.snippet {
            Some(snippet) => {
                let preview = zcode_editor::snippet::to_placeholders(snippet);
                let first = preview.lines().next().unwrap_or("").trim().to_string();
                let lines = preview.lines().count();
                vec![PickerItem {
                    label: first.into(),
                    detail: Some(if lines > 1 { format!("{lines} lines") } else { "Wrap the selection".into() }.into()),
                    icon: Some(IconName::Replace),
                    ..Default::default()
                }]
            }
            None => vec![],
        };
    }

    fn empty_message(&self) -> SharedString {
        "Type an Emmet abbreviation to wrap the selection in".into()
    }

    fn confirm(&mut self, _: usize, window: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        let Some(snippet) = self.snippet.clone() else { return false };
        let (editor, range) = (self.editor.clone(), self.range.clone());
        self.workspace
            .update(cx, |ws, cx| {
                ws.dismiss_modal(window, cx);
                editor.update(cx, |state, cx| state.transact(window, cx, |state, window, cx| state.insert_snippet(range, &snippet, window, cx)));
            })
            .ok();
        false
    }
}

// ---------------------------------------------------------------------------
// Rename file or folder (Project navigator).

pub struct RenameFileDelegate {
    workspace: WeakEntity<Workspace>,
    path: PathBuf,
    original: String,
    new_name: String,
    items: Vec<PickerItem>,
}

impl RenameFileDelegate {
    pub fn new(workspace: WeakEntity<Workspace>, path: PathBuf) -> Self {
        let original = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        Self {
            workspace,
            path,
            new_name: original.clone(),
            original,
            items: vec![],
        }
    }
}

impl PickerDelegate for RenameFileDelegate {
    fn placeholder(&self) -> SharedString {
        "New name, or a path to move it to".into()
    }

    fn items(&self) -> &[PickerItem] {
        &self.items
    }

    fn empty_message(&self) -> SharedString {
        format!("Type a new name for “{}”", self.original).into()
    }

    fn update_matches(&mut self, query: String, _: &mut Window, _: &mut Context<Picker<Self>>) {
        self.new_name = query.trim().trim_end_matches('/').to_string();
        // A path relative to the item's folder moves it there.
        let valid = !self.new_name.is_empty() && self.new_name != self.original && !self.new_name.starts_with('/');
        let verb = if self.new_name.contains('/') { "Move" } else { "Rename" };
        self.items = if valid {
            vec![PickerItem {
                label: format!("{verb} “{}” to “{}”", self.original, self.new_name).into(),
                icon: Some(IconName::Pencil),
                ..Default::default()
            }]
        } else {
            vec![]
        };
    }

    fn confirm(&mut self, _: usize, window: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        let (path, name) = (self.path.clone(), self.new_name.clone());
        self.workspace
            .update(cx, |ws, cx| {
                ws.dismiss_modal(window, cx);
                ws.rename_path(path, name, window, cx);
            })
            .ok();
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{flatten_symbols, split_line_suffix};
    use serde_json::json;

    #[test]
    fn flattens_hierarchical_symbols() {
        let value = json!([{
            "name": "Counter", "kind": 23,
            "range": {"start": {"line": 3, "character": 0}, "end": {"line": 12, "character": 1}},
            "selectionRange": {"start": {"line": 3, "character": 7}, "end": {"line": 3, "character": 14}},
            "children": [{
                "name": "increment(by:)", "kind": 6,
                "range": {"start": {"line": 6, "character": 4}, "end": {"line": 8, "character": 5}},
                "selectionRange": {"start": {"line": 6, "character": 18}, "end": {"line": 6, "character": 27}},
            }]
        }]);
        let entries = flatten_symbols(value);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].depth, 1);
        assert_eq!(entries[1].selection_range.start.line, 6);
    }

    #[test]
    fn line_suffix() {
        assert_eq!(split_line_suffix("Counter.swift:12"), ("Counter.swift", Some(12)));
        assert_eq!(split_line_suffix("Counter"), ("Counter", None));
    }
}

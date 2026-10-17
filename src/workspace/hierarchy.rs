//! Find ▸ Find Call Hierarchy (⌃⇧⌘H), Find Ancestor Types, Find Descendent
//! Types and Find Conforming Types: the symbol at the focused pane's cursor
//! as the root of a tree in the Find navigator, as Xcode shows them. The
//! call hierarchy shows callers, as Xcode's does, or what the symbol calls
//! (Zed's outgoing calls); the type hierarchy shows supertypes or subtypes.
//! Each level is asked for when it's expanded. Editor ▸ Show Callers… lists
//! the callers in a menu at the cursor.
//!
//! Clicking a caller opens the call, in the caller; clicking anything else
//! opens its declaration.

use std::collections::HashMap;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_kit::assets::IconName;

use super::navigator::{NavigatorTab, empty, row};
use super::pickers::symbol_icon;
use super::{Target, Workspace};
use crate::lsp::hierarchy::{self, HierarchyItem, HierarchyKind};
use crate::lsp::offset_to_lsp;

/// The tree in the Find navigator.
pub struct Hierarchy {
    pub kind: HierarchyKind,
    pub root: Node,
    /// The node selected last (indices from the root).
    selected: Option<Vec<usize>>,
    /// Requests in flight, by node; replacing the tree drops (cancels) them.
    tasks: HashMap<Vec<usize>, Task<()>>,
}

pub struct Node {
    pub item: HierarchyItem,
    /// `None` until asked for.
    pub children: Option<Vec<Node>>,
    pub expanded: bool,
    pub error: Option<String>,
}

impl Node {
    fn new(item: HierarchyItem) -> Self {
        Self { item, children: None, expanded: false, error: None }
    }

    fn at(&self, path: &[usize]) -> Option<&Node> {
        match path.split_first() {
            None => Some(self),
            Some((ix, rest)) => self.children.as_ref()?.get(*ix)?.at(rest),
        }
    }

    fn at_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        match path.split_first() {
            None => Some(self),
            Some((ix, rest)) => self.children.as_mut()?.get_mut(*ix)?.at_mut(rest),
        }
    }
}

/// What the Find navigator shows: text results, or a hierarchy.
#[derive(Default)]
pub struct HierarchyState {
    pub tree: Option<Hierarchy>,
    /// The hierarchy is shown (it stays, hidden, while text results are).
    pub shown: bool,
    /// Asking for the symbol at the cursor; replacing it cancels it.
    prepare: Option<Task<()>>,
    pub message: Option<String>,
    /// The server the hierarchy is from: its nodes expand there.
    server: Option<std::sync::Arc<crate::lsp::LanguageServer>>,
}

/// A row of the tree as shown: depth, path from the root, node.
struct Row<'a> {
    depth: usize,
    path: Vec<usize>,
    node: &'a Node,
}

fn rows(root: &Node) -> Vec<Row<'_>> {
    fn walk<'a>(node: &'a Node, depth: usize, path: Vec<usize>, out: &mut Vec<Row<'a>>) {
        let children = node.expanded.then_some(node.children.as_ref()).flatten();
        out.push(Row { depth, path: path.clone(), node });
        for (ix, child) in children.into_iter().flatten().enumerate() {
            let mut path = path.clone();
            path.push(ix);
            walk(child, depth + 1, path, out);
        }
    }
    let mut out = vec![];
    walk(root, 0, vec![], &mut out);
    out
}

pub fn title(kind: HierarchyKind) -> &'static str {
    match kind {
        HierarchyKind::Callers | HierarchyKind::Callees => "Call Hierarchy",
        HierarchyKind::Supertypes | HierarchyKind::Subtypes => "Type Hierarchy",
    }
}

impl Workspace {
    /// Starts a hierarchy of `kind` at the focused pane's cursor.
    pub fn find_hierarchy(&mut self, kind: HierarchyKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.active_item() else { return };
        let path = item.path.clone();
        let (text, cursor) = {
            let state = item.editor.read(cx);
            (state.text().clone(), state.cursor())
        };
        let capability = if kind.is_calls() { "/callHierarchyProvider" } else { "/typeHierarchyProvider" };
        let Some(server) = self.lsp.update(cx, |lsp, _| lsp.synced_server_supporting(&path, None, Some(capability))) else {
            if self.lsp.update(cx, |lsp, cx| lsp.synced_server(&path, cx)).is_some() {
                let what = if kind.is_calls() { "call hierarchies" } else { "type hierarchies" };
                self.notify_error(format!("The language server doesn’t offer {what}."), window, cx);
            } else {
                self.lsp_unavailable(window, cx);
            }
            return;
        };
        self.hierarchy.server = Some(server.clone());
        self.navigator = NavigatorTab::Find;
        self.show_navigator = true;
        self.hierarchy.shown = true;
        self.hierarchy.tree = None;
        self.hierarchy.message = Some("Searching…".into());
        let position = offset_to_lsp(&text, cursor);
        self.hierarchy.prepare = Some(cx.spawn(async move |this, cx| {
            let items = hierarchy::prepare(server, kind, path, position).await;
            this.update(cx, |this, cx| {
                this.hierarchy.prepare = None;
                match items {
                    Ok(items) if !items.is_empty() => {
                        let root = items.into_iter().next().expect("not empty");
                        this.show_hierarchy(kind, root, cx);
                    }
                    Ok(_) => {
                        let what = if kind.is_calls() { "a function or method" } else { "a type" };
                        // sourcekit-lsp answers from its index, which a new project builds first.
                        this.hierarchy.message = Some(match this.lsp.read(cx).progress_message() {
                            Some(progress) => format!("Nothing yet: sourcekit-lsp is still indexing ({progress}). Try again when it’s done."),
                            None => format!("The cursor isn’t on {what} the index knows."),
                        });
                    }
                    Err(err) => this.hierarchy.message = Some(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Shows `root`'s hierarchy of `kind`, its first level expanded.
    fn show_hierarchy(&mut self, kind: HierarchyKind, root: HierarchyItem, cx: &mut Context<Self>) {
        let mut root = Node::new(root);
        // A root has no call sites of its own.
        root.item.call_sites.clear();
        self.hierarchy.message = None;
        self.hierarchy.shown = true;
        self.hierarchy.tree = Some(Hierarchy { kind, root, selected: Some(vec![]), tasks: HashMap::new() });
        self.expand_hierarchy_node(vec![], cx);
    }

    /// Callers ⇄ callees, supertypes ⇄ subtypes: the same root, the other way.
    pub fn set_hierarchy_kind(&mut self, kind: HierarchyKind, cx: &mut Context<Self>) {
        let Some(tree) = self.hierarchy.tree.as_ref() else { return };
        if tree.kind == kind || tree.kind.is_calls() != kind.is_calls() {
            return;
        }
        let root = tree.root.item.clone();
        self.show_hierarchy(kind, root, cx);
    }

    /// Expands a node, asking for its children the first time.
    pub(super) fn expand_hierarchy_node(&mut self, path: Vec<usize>, cx: &mut Context<Self>) {
        let Some(tree) = self.hierarchy.tree.as_mut() else { return };
        let kind = tree.kind;
        let Some(node) = tree.root.at_mut(&path) else { return };
        node.expanded = true;
        if node.children.is_some() || tree.tasks.contains_key(&path) {
            cx.notify();
            return;
        }
        let item = node.item.clone();
        let Some(server) = self.hierarchy.server.clone().filter(|s| s.is_alive()) else { return };
        let key = path.clone();
        let task = cx.spawn(async move |this, cx| {
            let children = hierarchy::children(server, kind, &item).await;
            this.update(cx, |this, cx| {
                let Some(tree) = this.hierarchy.tree.as_mut() else { return };
                tree.tasks.remove(&path);
                let Some(node) = tree.root.at_mut(&path) else { return };
                match children {
                    Ok(children) => node.children = Some(children.into_iter().map(Node::new).collect()),
                    Err(err) => {
                        node.children = Some(vec![]);
                        node.error = Some(format!("{err:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        });
        tree.tasks.insert(key, task);
        cx.notify();
    }

    fn toggle_hierarchy_node(&mut self, path: Vec<usize>, cx: &mut Context<Self>) {
        let expanded = self.hierarchy.tree.as_ref().and_then(|tree| tree.root.at(&path)).is_some_and(|node| node.expanded);
        if expanded {
            if let Some(node) = self.hierarchy.tree.as_mut().and_then(|tree| tree.root.at_mut(&path)) {
                node.expanded = false;
            }
            cx.notify();
        } else {
            self.expand_hierarchy_node(path, cx);
        }
    }

    /// Opens a node: a caller at its (first) call, anything else at its
    /// declaration.
    fn open_hierarchy_node(&mut self, path: Vec<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tree) = self.hierarchy.tree.as_mut() else { return };
        let kind = tree.kind;
        let Some(node) = tree.root.at(&path) else { return };
        let item = node.item.clone();
        tree.selected = Some(path.clone());
        let range = match (kind, item.call_sites.first()) {
            (HierarchyKind::Callers, Some(call)) if !path.is_empty() => *call,
            _ => item.selection,
        };
        self.open_file(item.path, Some(Target::LspRange(range)), true, window, cx);
        cx.notify();
    }

    /// Makes a node the root of a new hierarchy of the same kind (its
    /// context menu's Find Call Hierarchy, Find Ancestor Types…).
    fn reroot_hierarchy(&mut self, path: Vec<usize>, kind: HierarchyKind, cx: &mut Context<Self>) {
        let Some(item) = self.hierarchy.tree.as_ref().and_then(|tree| tree.root.at(&path)).map(|node| node.item.clone()) else { return };
        self.show_hierarchy(kind, item, cx);
    }

    /// Editor ▸ Show Callers…: the callers of the symbol at the cursor, in a
    /// menu at the cursor; choosing one opens the call.
    pub fn show_callers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.active_item() else { return };
        let path = item.path.clone();
        let editor = item.editor.clone();
        let (text, cursor) = {
            let state = editor.read(cx);
            (state.text().clone(), state.cursor())
        };
        let Some(server) = self.lsp.update(cx, |lsp, _| lsp.synced_server_supporting(&path, None, Some("/callHierarchyProvider"))) else {
            if self.lsp.update(cx, |lsp, cx| lsp.synced_server(&path, cx)).is_some() {
                self.notify_error("The language server doesn’t offer call hierarchies.".into(), window, cx);
            } else {
                self.lsp_unavailable(window, cx);
            }
            return;
        };
        let position = offset_to_lsp(&text, cursor);
        self.hierarchy.prepare = Some(cx.spawn_in(window, async move |this, cx| {
            let callers = async {
                let root = hierarchy::prepare(server.clone(), HierarchyKind::Callers, path, position).await?.into_iter().next();
                match root {
                    Some(root) => hierarchy::children(server, HierarchyKind::Callers, &root).await,
                    None => Ok(vec![]),
                }
            }
            .await
            .unwrap_or_default();
            this.update_in(cx, |this, window, cx| {
                this.hierarchy.prepare = None;
                let at = editor.read(cx).range_to_bounds(&(cursor..cursor)).map(|b| b.bottom_left()).unwrap_or_default();
                let workspace = cx.weak_entity();
                this.show_popup_menu(at, window, cx, move |mut menu, _, _| {
                    if callers.is_empty() {
                        return menu.item(PopupMenuItem::new("No Callers").disabled(true));
                    }
                    for caller in callers {
                        let line = caller.call_sites.first().unwrap_or(&caller.selection).start.line + 1;
                        let label = format!("{} — {}:{line}", caller.name, super::pane::file_title(&caller.path));
                        let workspace = workspace.clone();
                        menu = menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                            let range = *caller.call_sites.first().unwrap_or(&caller.selection);
                            let path = caller.path.clone();
                            workspace.update(cx, |this, cx| this.open_file(path, Some(Target::LspRange(range)), true, window, cx)).ok();
                        }));
                    }
                    menu
                });
            })
            .ok();
        }));
    }

    /// The control channel's `hierarchy` command: `kind callers|callees|
    /// supertypes|subtypes`, `expand <row>`, `collapse <row>`, `open <row>`,
    /// `reroot <row>`, `close` (rows as in the dump, from 0).
    pub fn hierarchy_command(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let (verb, rest) = arg.split_once(' ').unwrap_or((arg, ""));
        let row_path = |this: &Self| -> Result<Vec<usize>, String> {
            let tree = this.hierarchy.tree.as_ref().ok_or("no hierarchy")?;
            let ix: usize = rest.trim().parse().map_err(|_| "usage: <verb> <row>")?;
            rows(&tree.root).into_iter().nth(ix).map(|row| row.path).ok_or(format!("no row {ix}"))
        };
        match verb {
            "kind" => {
                let kind = match rest {
                    "callers" => HierarchyKind::Callers,
                    "callees" => HierarchyKind::Callees,
                    "supertypes" => HierarchyKind::Supertypes,
                    "subtypes" => HierarchyKind::Subtypes,
                    other => return Err(format!("unknown kind {other}")),
                };
                self.set_hierarchy_kind(kind, cx);
            }
            "expand" => {
                let path = row_path(self)?;
                self.expand_hierarchy_node(path, cx);
            }
            "collapse" => {
                let path = row_path(self)?;
                self.toggle_hierarchy_node(path, cx);
            }
            "open" => {
                let path = row_path(self)?;
                // Opening focuses an editor: not from inside this update.
                let workspace = cx.weak_entity();
                window.defer(cx, move |window, cx| {
                    workspace.update(cx, |this, cx| this.open_hierarchy_node(path, window, cx)).ok();
                });
            }
            "reroot" => {
                let path = row_path(self)?;
                let kind = self.hierarchy.tree.as_ref().map(|tree| tree.kind).ok_or("no hierarchy")?;
                self.reroot_hierarchy(path, kind, cx);
            }
            "close" => {
                self.hierarchy.shown = false;
                cx.notify();
            }
            other => return Err(format!("unknown hierarchy command {other}")),
        }
        Ok(())
    }

    /// The active editor's signature help as shown, the active argument in
    /// brackets.
    pub(super) fn signature_help_summary(&self, cx: &App) -> serde_json::Value {
        let Some(editor) = self.active_editor() else { return serde_json::Value::Null };
        let Some(state) = editor.read(cx).signature_help() else { return serde_json::Value::Null };
        let help = &state.help;
        let Some(signature) = help.signatures.get(help.active_signature.unwrap_or(0) as usize) else {
            return serde_json::Value::Null;
        };
        let (before, parameter, after) = zcode_editor::split_signature_label(signature, signature.active_parameter.or(help.active_parameter));
        serde_json::json!(if parameter.is_empty() { before } else { format!("{before}[{parameter}]{after}") })
    }

    pub(super) fn hierarchy_summary(&self) -> serde_json::Value {
        let tree = self.hierarchy.tree.as_ref();
        serde_json::json!({
            "shown": self.hierarchy.shown,
            "message": self.hierarchy.message,
            "kind": tree.map(|tree| format!("{:?}", tree.kind)),
            "loading": tree.is_some_and(|tree| !tree.tasks.is_empty()) || self.hierarchy.prepare.is_some(),
            // Each row: indentation, name, where.
            "rows": tree.map(|tree| rows(&tree.root).iter().map(|row| {
                format!("{}{} — {}", "  ".repeat(row.depth), row.node.item.name, location_label(tree.kind, row))
            }).collect::<Vec<_>>()),
        })
    }

    pub(super) fn render_hierarchy(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let close = Button::new("hierarchy-close")
            .icon(IconName::Close)
            .ghost()
            .xsmall()
            .tooltip("Show Find Results")
            .on_click(cx.listener(|this, _, _, cx| {
                this.hierarchy.shown = false;
                cx.notify();
            }));
        let Some(tree) = self.hierarchy.tree.as_ref() else {
            let message = self.hierarchy.message.clone().unwrap_or_default();
            return v_flex()
                .size_full()
                .child(h_flex().px_2().py_1().child(div().flex_1()).child(close))
                .child(empty(message, cx))
                .into_any_element();
        };
        let kind = tree.kind;
        let (first, second) = if kind.is_calls() {
            ((HierarchyKind::Callers, "Callers"), (HierarchyKind::Callees, "Callees"))
        } else {
            ((HierarchyKind::Supertypes, "Supertypes"), (HierarchyKind::Subtypes, "Subtypes"))
        };
        let toggle = |(choice, label): (HierarchyKind, &'static str)| {
            Button::new(SharedString::from(format!("hierarchy-{label}")))
                .label(label)
                .xsmall()
                .ghost()
                .selected(kind == choice)
                .on_click(cx.listener(move |this, _, _, cx| this.set_hierarchy_kind(choice, cx)))
        };
        let header = h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .child(div().flex_1().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme.muted_foreground).child(title(kind)))
            .child(toggle(first))
            .child(toggle(second))
            .child(close);

        let selected = tree.selected.clone();
        let mut list = v_flex().id("hierarchy").flex_1().min_h_0().overflow_y_scroll();
        for (ix, entry) in rows(&tree.root).into_iter().enumerate() {
            let node = entry.node;
            let loading = tree.tasks.contains_key(&entry.path);
            let leaf = node.children.as_ref().is_some_and(|c| c.is_empty());
            let toggle_path = entry.path.clone();
            let chevron = div().id(("hierarchy-chevron", ix)).flex_none().w(px(14.)).when(!leaf, |el| {
                el.child(
                    Icon::new(if node.expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                        .xsmall()
                        .text_color(theme.muted_foreground),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_hierarchy_node(toggle_path.clone(), cx);
                }))
            });
            let location = location_label(kind, &entry);
            let open_path = entry.path.clone();
            let menu_path = entry.path.clone();
            let workspace = cx.weak_entity();
            list = list.child(
                row(("hierarchy-row", ix))
                    .pl(px(8. + entry.depth as f32 * 14.))
                    .text_xs()
                    .when(selected.as_ref() == Some(&entry.path), |el| el.bg(theme.sidebar_accent))
                    .hover(|el| el.bg(theme.sidebar_accent))
                    .child(chevron)
                    .child(Icon::new(symbol_icon(node.item.kind)).xsmall().text_color(theme.muted_foreground))
                    .child(div().truncate().when(entry.depth == 0, |el| el.font_weight(FontWeight::SEMIBOLD)).child(node.item.name.clone()))
                    .child(div().flex_1().truncate().text_color(theme.muted_foreground).child(location))
                    .when(loading, |el| el.child(div().text_color(theme.muted_foreground).child("…")))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if event.click_count() > 1 {
                            this.toggle_hierarchy_node(open_path.clone(), cx);
                        } else {
                            this.open_hierarchy_node(open_path.clone(), window, cx);
                        }
                    }))
                    .context_menu(move |menu, _, _| {
                        let (workspace, path) = (workspace.clone(), menu_path.clone());
                        let entries: &[(&str, HierarchyKind)] = if kind.is_calls() {
                            &[("Find Call Hierarchy", HierarchyKind::Callers)]
                        } else {
                            &[("Find Ancestor Types", HierarchyKind::Supertypes), ("Find Descendent Types", HierarchyKind::Subtypes)]
                        };
                        entries.iter().fold(menu, |menu, &(label, kind)| {
                            let (workspace, path) = (workspace.clone(), path.clone());
                            menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                                workspace.update(cx, |this, cx| this.reroot_hierarchy(path.clone(), kind, cx)).ok();
                            }))
                        })
                    }),
            );
            if node.expanded
                && let Some(message) = node.error.clone().or_else(|| {
                    node.children.as_ref().is_some_and(|c| c.is_empty()).then(|| empty_label(kind).to_string())
                })
            {
                list = list.child(
                    div()
                        .pl(px(8. + (entry.depth + 1) as f32 * 14. + 14.))
                        .py(px(3.))
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(message),
                );
            }
        }
        v_flex().size_full().child(header).child(div().h(px(1.)).bg(theme.border)).child(list).into_any_element()
    }
}

fn empty_label(kind: HierarchyKind) -> &'static str {
    match kind {
        HierarchyKind::Callers => "No callers",
        HierarchyKind::Callees => "No calls",
        HierarchyKind::Supertypes => "No supertypes",
        HierarchyKind::Subtypes => "No subtypes",
    }
}

/// Where a row is: a caller's file and the line of its call (and how many
/// calls), a declaration's file and line, or an SDK symbol's module.
fn location_label(kind: HierarchyKind, row: &Row) -> String {
    let item = &row.node.item;
    if let Some(module) = item.interface_module() {
        return module;
    }
    let file = super::pane::file_title(&item.path);
    let calls = &item.call_sites;
    match (kind, calls.first()) {
        (HierarchyKind::Callers, Some(call)) if row.depth > 0 => {
            let more = if calls.len() > 1 { format!(" ({} calls)", calls.len()) } else { String::new() };
            format!("{file}:{}{more}", call.start.line + 1)
        }
        _ => format!("{file}:{}", item.selection.start.line + 1),
    }
}

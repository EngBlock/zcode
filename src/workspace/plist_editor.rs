//! The property list editor: Xcode's Key / Type / Value outline over a
//! property list's text. Every change is one replacement in that text
//! (`plist::edit`), made through its `EditorState`, so undo, redo, the
//! dirty dot and ⌘S behave as they do for the same file opened as source
//! code. Key names, types and allowed values come from Xcode's definitions
//! (`plist::schema`); Editor ▸ Raw Keys and Values shows the raw ones.

use std::cell::Cell;
use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState, Input, InputEvent, InputState, Redo, Undo};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::{
    AddPlistItem, DeletePlistItem, MoveLineDown, MoveLineUp, OpenAsSourceCode, PickerConfirm, PickerDismiss, PickerSelectNext,
    PickerSelectPrevious, PlistCollapse, PlistExpand, SetPlistTypeArray, SetPlistTypeBoolean, SetPlistTypeData, SetPlistTypeDate,
    SetPlistTypeDictionary, SetPlistTypeNumber, SetPlistTypeString, ToggleRawKeysAndValues,
};
use crate::plist::edit::{Document, TextEdit, unique_key};
use crate::plist::schema::{Resolved, Schema, default_value};
use crate::plist::{self, Kind, Node, Value};

/// Editor ▸ Raw Keys and Values, for every property list editor.
static RAW_KEYS: AtomicBool = AtomicBool::new(false);

pub fn raw_keys() -> bool {
    RAW_KEYS.load(Ordering::Relaxed)
}

pub enum PlistEditorEvent {
    /// The text changed (an edit here, undo, redo).
    Edited,
    /// A build-setting row (Info tab) was edited; `None` removes it.
    SetBuildSetting { setting: String, value: Option<String> },
    /// Open As ▸ Source Code.
    OpenAsSource,
}

/// An `INFOPLIST_KEY_` build setting shown among a target's Info.plist
/// keys, as Xcode's Info tab merges them.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingRow {
    /// The Info.plist key (`CFBundleDisplayName`).
    pub key: String,
    /// The build setting (`INFOPLIST_KEY_CFBundleDisplayName`).
    pub setting: String,
    /// `None` when configurations differ.
    pub value: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
    Key,
    Value,
}

/// One visible line of the outline.
#[derive(Clone)]
struct Row {
    /// Child indices from the root; empty for the root.
    path: Vec<usize>,
    depth: usize,
    /// The key of a dictionary's entry.
    key: Option<String>,
    /// The index of an array's item.
    index: Option<usize>,
    kind: Kind,
    value: Value,
    resolved: Resolved,
    /// The class of the container it's in.
    parent: Resolved,
    expanded: bool,
    /// For build-setting rows: the index in `settings`.
    setting: Option<usize>,
}

struct CellEdit {
    path: Vec<usize>,
    column: Column,
    setting: Option<usize>,
    input: Entity<InputState>,
    /// The highlighted key suggestion.
    highlighted: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

/// A row being dragged to a new position among its siblings.
#[derive(Clone)]
struct DraggedRow {
    path: Vec<usize>,
    label: SharedString,
}

impl Render for DraggedRow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_2()
            .py_0p5()
            .text_sm()
            .rounded(theme.radius)
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .child(self.label.clone())
    }
}

pub struct PlistEditor {
    focus_handle: FocusHandle,
    /// The text being edited (a tab's buffer, or the Info tab's file).
    text: Entity<EditorState>,
    schema: Option<&'static Schema>,
    /// The text `root` was parsed from.
    source: String,
    root: Result<Node, String>,
    stale: bool,
    /// Rows shown, rebuilt when the tree, expansion or raw keys change.
    rows: Rc<Vec<Row>>,
    rows_raw: Option<bool>,
    rows_stale: bool,
    /// Expanded rows by key path.
    expanded: HashSet<String>,
    selected: Option<Vec<usize>>,
    editing: Option<CellEdit>,
    /// The root row (hidden in the Info tab, which lists keys directly).
    show_root: bool,
    settings: Vec<SettingRow>,
    error: Option<String>,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PlistEditorEvent> for PlistEditor {}

impl Focusable for PlistEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

const ROW_HEIGHT: f32 = 22.;
const TYPE_WIDTH: f32 = 104.;
const INDENT: f32 = 14.;

/// A path's identity for expansion: keys and `#index`es from the root.
fn path_id(root: &Node, path: &[usize]) -> String {
    let mut id = String::new();
    let mut node = root;
    for &ix in path {
        let Some((key, child)) = node.child(ix) else { break };
        id.push('\u{1f}');
        match key {
            Some(key) => id.push_str(key),
            None => id.push_str(&format!("#{ix}")),
        }
        node = child;
    }
    id
}

impl PlistEditor {
    pub fn new(text: Entity<EditorState>, schema: Option<&'static Schema>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&text, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.stale = true;
                cx.emit(PlistEditorEvent::Edited);
                cx.notify();
            }
        });
        let mut expanded = HashSet::new();
        expanded.insert(String::new());
        Self {
            focus_handle: cx.focus_handle(),
            text,
            schema,
            source: String::new(),
            root: Err(String::new()),
            stale: true,
            rows: Rc::new(vec![]),
            rows_raw: None,
            rows_stale: true,
            expanded,
            selected: None,
            editing: None,
            show_root: true,
            settings: vec![],
            error: None,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: vec![subscription],
        }
    }

    /// For a target's Info tab: no root row, and the target's
    /// `INFOPLIST_KEY_` build settings listed with the file's keys.
    pub fn hide_root(mut self) -> Self {
        self.show_root = false;
        self
    }

    pub fn set_settings(&mut self, settings: Vec<SettingRow>, cx: &mut Context<Self>) {
        if self.settings != settings {
            self.settings = settings;
            self.rows_stale = true;
            cx.notify();
        }
    }

    /// The text was replaced without a change event (reloaded from disk).
    pub fn text_replaced(&mut self, cx: &mut Context<Self>) {
        self.stale = true;
        self.editing = None;
        cx.notify();
    }

    pub fn text_entity(&self) -> &Entity<EditorState> {
        &self.text
    }

    /// The parse of the current text.
    fn refresh(&mut self, cx: &App) {
        if !self.stale {
            return;
        }
        self.stale = false;
        let text = self.text.read(cx).text().to_string();
        if text != self.source || self.root.is_err() {
            self.root = plist::parse_xml(&text).map_err(|err| format!("{err:#}"));
            self.source = text;
            self.rows_stale = true;
            if let (Some(path), Ok(root)) = (&self.selected, &self.root)
                && root.at(path).is_none()
            {
                // Keep the selection on the nearest row that still exists.
                let mut path = path.clone();
                while !path.is_empty() && root.at(&path).is_none() {
                    if let Some(last) = path.last_mut()
                        && *last > 0
                    {
                        *last -= 1;
                    } else {
                        path.pop();
                    }
                }
                self.selected = Some(path);
            }
        }
    }

    fn rows(&mut self, cx: &App) -> Rc<Vec<Row>> {
        self.refresh(cx);
        let raw = raw_keys();
        if self.rows_stale || self.rows_raw != Some(raw) {
            self.rows = Rc::new(self.build_rows());
            self.rows_stale = false;
            self.rows_raw = Some(raw);
        }
        self.rows.clone()
    }

    fn build_rows(&self) -> Vec<Row> {
        match &self.root {
            Ok(root) => self.build_rows_from(root),
            Err(_) => vec![],
        }
    }

    fn build_rows_from(&self, root: &Node) -> Vec<Row> {
        let root_class = self.schema.map_or(Resolved::UNKNOWN, |s| s.root());
        let mut rows = vec![];
        let base = if self.show_root {
            rows.push(Row {
                path: vec![],
                depth: 0,
                key: None,
                index: None,
                kind: root.value.kind(),
                value: root.value.clone(),
                resolved: root_class,
                parent: Resolved::UNKNOWN,
                expanded: self.expanded.contains(""),
                setting: None,
            });
            1
        } else {
            0
        };
        if !self.show_root || self.expanded.contains("") {
            self.push_children(&mut rows, root, &[], base, root_class, root);
            if matches!(root.value, Value::Dict(_)) {
                for (ix, setting) in self.settings.iter().enumerate() {
                    let resolved = self.schema.map_or(Resolved::UNKNOWN, |s| s.child(&root_class, Some(&setting.key)));
                    let value = setting_value(setting, &resolved);
                    rows.push(Row {
                        path: vec![],
                        depth: base,
                        key: Some(setting.key.clone()),
                        index: None,
                        kind: value.kind(),
                        value,
                        resolved,
                        parent: root_class,
                        expanded: false,
                        setting: Some(ix),
                    });
                }
            }
        }
        rows
    }

    fn push_children(&self, rows: &mut Vec<Row>, node: &Node, path: &[usize], depth: usize, class: Resolved, root: &Node) {
        for ix in 0..node.children_len() {
            let Some((key, child)) = node.child(ix) else { continue };
            let resolved = self.schema.map_or(Resolved::UNKNOWN, |s| s.child(&class, key));
            let mut child_path = path.to_vec();
            child_path.push(ix);
            let expanded = child.value.kind().is_container() && self.expanded.contains(&path_id(root, &child_path));
            rows.push(Row {
                path: child_path.clone(),
                depth,
                key: key.map(str::to_string),
                index: key.is_none().then_some(ix),
                kind: child.value.kind(),
                value: child.value.clone(),
                resolved,
                parent: class,
                expanded,
                setting: None,
            });
            if expanded {
                self.push_children(rows, child, &child_path, depth + 1, resolved, root);
            }
        }
    }

    fn key_label(&self, row: &Row, raw: bool) -> String {
        if row.setting.is_none() && row.path.is_empty() {
            return match self.schema {
                Some(schema) if !raw => schema.root_label.to_string(),
                _ => "Root".to_string(),
            };
        }
        if let Some(ix) = row.index {
            return format!("Item {ix}");
        }
        let key = row.key.clone().unwrap_or_default();
        if raw {
            return key;
        }
        row.parent.key(&key).map(|k| k.label).filter(|l| !l.is_empty()).map_or(key, str::to_string)
    }

    fn value_label(row: &Row, raw: bool) -> String {
        if let Value::String(s) = &row.value
            && !raw
            && let Some(choice) = row.resolved.choices().iter().find(|c| c.value == s)
        {
            return choice.label.to_string();
        }
        row.value.display()
    }

    // ------------------------------------------------------------------
    // Edits

    /// Applies one text edit through the editor, so it's one undo step.
    fn apply(&mut self, edit: TextEdit, window: &mut Window, cx: &mut Context<Self>) {
        self.text.update(cx, |state, cx| {
            state.set_selected_range(edit.range.clone(), cx);
            state.replace(edit.text, window, cx);
            state.set_selected_range(0..0, cx);
        });
        self.stale = true;
        self.error = None;
        self.refresh(cx);
        cx.notify();
    }

    fn with_document<R>(&mut self, cx: &App, f: impl FnOnce(&Document) -> Option<R>) -> Option<R> {
        self.refresh(cx);
        let root = self.root.as_ref().ok()?;
        f(&Document::new(&self.source, root))
    }

    fn fail(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        cx.notify();
    }

    fn node(&self, path: &[usize]) -> Option<&Node> {
        self.root.as_ref().ok()?.at(path)
    }

    fn class_at(&self, path: &[usize]) -> (Resolved, Resolved) {
        let (Some(schema), Ok(root)) = (self.schema, &self.root) else { return (Resolved::UNKNOWN, Resolved::UNKNOWN) };
        let along = schema.along(root, path);
        let own = along.last().copied().unwrap_or(Resolved::UNKNOWN);
        let parent = if along.len() >= 2 { along[along.len() - 2] } else { Resolved::UNKNOWN };
        (own, parent)
    }

    fn expand(&mut self, path: &[usize], expanded: bool) {
        let Ok(root) = &self.root else { return };
        let id = path_id(root, path);
        if expanded {
            self.expanded.insert(id);
        } else {
            self.expanded.remove(&id);
        }
        self.rows_stale = true;
    }

    /// The + on a row: a new entry after it, or first in it when it's an
    /// expanded container (or the root).
    pub fn add_item(&mut self, after: Option<Vec<usize>>, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_edit(window, cx);
        self.refresh(cx);
        let Some(root) = self.root.as_ref().ok() else { return };
        let path = after.unwrap_or_default();
        let Some(node) = root.at(&path) else { return };
        let into = path.is_empty()
            || (node.value.kind().is_container() && self.expanded.contains(&path_id(root, &path)))
            || (!self.show_root && path.is_empty());
        let (parent_path, index) = if into {
            (path.clone(), 0)
        } else {
            let (last, parent) = path.split_last().expect("not the root");
            (parent.to_vec(), last + 1)
        };
        let Some(parent) = root.at(&parent_path) else { return };
        let (parent_class, _) = self.class_at(&parent_path);
        let (key, value) = match &parent.value {
            Value::Dict(_) => (Some(unique_key(parent, "New item")), Value::String(String::new())),
            Value::Array(_) => {
                let element = self.schema.map_or(Resolved::UNKNOWN, |s| s.child(&parent_class, None));
                (None, default_value(&element))
            }
            _ => return,
        };
        let Some(edit) = self.with_document(cx, |doc| doc.insert(&parent_path, index, key.as_deref(), &value)) else { return };
        if !parent_path.is_empty() || self.show_root {
            self.expand(&parent_path, true);
        }
        self.apply(edit, window, cx);
        let mut new_path = parent_path;
        new_path.push(index);
        self.selected = Some(new_path.clone());
        let column = if key.is_some() { Column::Key } else { Column::Value };
        self.begin_edit(new_path, column, None, window, cx);
    }

    pub fn remove(&mut self, path: Vec<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        if path.is_empty() {
            return;
        }
        let Some(edit) = self.with_document(cx, |doc| doc.remove(&path)) else { return };
        self.apply(edit, window, cx);
        self.focus_handle.focus(window, cx);
    }

    fn remove_setting(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.editing = None;
        if let Some(setting) = self.settings.get(ix) {
            cx.emit(PlistEditorEvent::SetBuildSetting { setting: setting.setting.clone(), value: None });
        }
    }

    pub fn set_type(&mut self, path: Vec<usize>, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        let Some(node) = self.node(&path) else { return };
        if node.value.kind() == kind || (path.is_empty() && !kind.is_container()) {
            return;
        }
        let value = node.value.converted(kind);
        let Some(edit) = self.with_document(cx, |doc| doc.set_value(&path, &value)) else { return };
        self.apply(edit, window, cx);
    }

    pub fn set_value(&mut self, path: Vec<usize>, value: Value, window: &mut Window, cx: &mut Context<Self>) {
        if self.node(&path).is_some_and(|n| n.value == value) {
            return;
        }
        let Some(edit) = self.with_document(cx, |doc| doc.set_value(&path, &value)) else { return };
        self.apply(edit, window, cx);
    }

    /// Moves a row among its siblings (⌥⌘[ and ⌥⌘], or a drag).
    pub fn move_to(&mut self, path: Vec<usize>, to: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        let Some((&from, parent)) = path.split_last() else { return };
        let parent = parent.to_vec();
        let Some(edit) = self.with_document(cx, |doc| doc.move_child(&parent, from, to)) else { return };
        self.apply(edit, window, cx);
        let mut moved = parent;
        moved.push(to);
        self.selected = Some(moved);
    }

    fn move_selected(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.selected.clone().filter(|p| !p.is_empty()) else { return };
        let (&ix, parent) = path.split_last().expect("not empty");
        let count = self.node(parent).map_or(0, Node::children_len);
        let to = ix as isize + delta;
        if to >= 0 && (to as usize) < count {
            self.move_to(path, to as usize, window, cx);
        }
    }

    /// Commits a key typed for the entry at `path`: a suggestion's label
    /// stands for its raw key, and a value that's still blank takes the
    /// type Xcode gives that key.
    fn commit_key(&mut self, path: Vec<usize>, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        let typed = typed.trim();
        let Some((&ix, parent_path)) = path.split_last() else { return };
        let Some(parent) = self.node(parent_path) else { return };
        let Some((Some(old), node)) = parent.child(ix) else { return };
        let (_, parent_class) = self.class_at(&path);
        let key = parent_class
            .keys()
            .iter()
            .find(|k| k.name == typed || (!k.label.is_empty() && k.label.eq_ignore_ascii_case(typed)))
            .map_or(typed.to_string(), |k| k.name.to_string());
        if key.is_empty() || key == old {
            return;
        }
        if parent.get(&key).is_some() {
            self.fail(format!("The key “{key}” already exists."), cx);
            return;
        }
        let class = self.schema.map_or(Resolved::UNKNOWN, |s| s.child(&parent_class, Some(&key)));
        let blank = match &node.value {
            Value::String(s) => s.is_empty(),
            Value::Array(a) => a.is_empty(),
            Value::Dict(d) => d.is_empty(),
            _ => false,
        };
        let value = match class.kind {
            Some(kind) if blank && kind != node.value.kind() => Some(default_value(&class)),
            _ => None,
        };
        let path_clone = path.clone();
        let Some(edit) = self.with_document(cx, |doc| doc.set_entry(&path_clone, &key, value.as_ref())) else { return };
        self.apply(edit, window, cx);
    }

    fn commit_value(&mut self, path: Vec<usize>, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(node) = self.node(&path) else { return };
        let (class, _) = self.class_at(&path);
        // Allowed values show their labels; typing a label means its value.
        let typed = class
            .choices()
            .iter()
            .find(|c| c.label == typed.trim() && !raw_keys())
            .map_or(typed.to_string(), |c| c.value.to_string());
        match node.value.parse_as(&typed) {
            Ok(value) => self.set_value(path, value, window, cx),
            Err(err) => self.fail(err, cx),
        }
    }

    fn commit_setting(&mut self, ix: usize, typed: &str, cx: &mut Context<Self>) {
        let Some(setting) = self.settings.get(ix) else { return };
        let typed = typed.trim();
        let resolved = self.class_at(&[]).0;
        let class = self.schema.map_or(Resolved::UNKNOWN, |s| s.child(&resolved, Some(&setting.key)));
        let value = match class.kind {
            Some(Kind::Boolean) => match plist::parse_bool(typed) {
                Some(b) => if b { "YES" } else { "NO" }.to_string(),
                None => return self.fail(format!("“{typed}” isn’t YES or NO."), cx),
            },
            _ => typed.to_string(),
        };
        if setting.value.as_deref() != Some(value.as_str()) {
            let setting = setting.setting.clone();
            cx.emit(PlistEditorEvent::SetBuildSetting { setting, value: (!value.is_empty()).then_some(value) });
        }
    }

    // ------------------------------------------------------------------
    // Inline editing

    fn begin_edit(&mut self, path: Vec<usize>, column: Column, setting: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_edit(window, cx);
        let raw = raw_keys();
        let initial = match (setting, column) {
            (Some(ix), _) => self.settings.get(ix).and_then(|s| s.value.clone()).unwrap_or_default(),
            (None, Column::Key) => {
                let Some((&ix, parent)) = path.split_last() else { return };
                let Some((Some(key), _)) = self.node(parent).and_then(|p| p.child(ix)) else { return };
                let (_, parent_class) = self.class_at(&path);
                match parent_class.key(key) {
                    Some(k) if !raw && !k.label.is_empty() => k.label.to_string(),
                    _ => key.to_string(),
                }
            }
            (None, Column::Value) => {
                let Some(node) = self.node(&path) else { return };
                if node.value.kind().is_container() {
                    return;
                }
                let (class, _) = self.class_at(&path);
                match &node.value {
                    Value::String(s) if !raw => class.choices().iter().find(|c| c.value == s).map_or(s.clone(), |c| c.label.to_string()),
                    other => other.display(),
                }
            }
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(initial));
        let subscriptions = vec![cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::Blur => this.commit_edit(window, cx),
            InputEvent::Change => {
                if let Some(edit) = this.editing.as_mut() {
                    edit.highlighted = None;
                }
                cx.notify();
            }
            _ => {}
        })];
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        self.selected = Some(path.clone());
        self.editing = Some(CellEdit { path, column, setting, input, highlighted: None, _subscriptions: subscriptions });
        cx.notify();
    }

    /// Keys Xcode suggests while a key is typed: the container's known keys
    /// that aren't used yet, matching what's typed.
    fn suggestions(&self, cx: &App) -> Vec<(&'static str, &'static str)> {
        let Some(edit) = self.editing.as_ref().filter(|e| e.column == Column::Key && e.setting.is_none()) else { return vec![] };
        let Some((&ix, parent_path)) = edit.path.split_last() else { return vec![] };
        let Some(parent) = self.node(parent_path) else { return vec![] };
        let current = parent.child(ix).and_then(|(k, _)| k).unwrap_or("");
        let (_, parent_class) = self.class_at(&edit.path);
        let query = edit.input.read(cx).value().trim().to_lowercase();
        let showing_current = parent_class.key(current).is_some_and(|k| k.label.eq_ignore_ascii_case(&query)) || query == current.to_lowercase();
        let mut out: Vec<(&'static str, &'static str)> = parent_class
            .keys()
            .iter()
            .filter(|k| k.name == current || parent.get(k.name).is_none())
            .filter(|k| showing_current || query.is_empty() || k.label.to_lowercase().contains(&query) || k.name.to_lowercase().contains(&query))
            .map(|k| (k.name, k.label))
            .collect();
        out.sort_by_key(|(name, label)| if label.is_empty() { name.to_lowercase() } else { label.to_lowercase() });
        out.dedup_by_key(|(name, _)| *name);
        out
    }

    fn commit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.editing.take() else { return };
        // The field goes away; keep the keyboard in the outline.
        if edit.input.read(cx).focus_handle(cx).is_focused(window) {
            self.focus_handle.focus(window, cx);
        }
        let mut typed = edit.input.read(cx).value().to_string();
        if let Some(ix) = edit.highlighted
            && let Some((name, _)) = self.editing_suggestions_for(&edit, cx).get(ix)
        {
            typed = name.to_string();
        }
        match (edit.setting, edit.column) {
            (Some(ix), _) => self.commit_setting(ix, &typed, cx),
            (None, Column::Key) => self.commit_key(edit.path, &typed, window, cx),
            (None, Column::Value) => self.commit_value(edit.path, &typed, window, cx),
        }
        cx.notify();
    }

    fn editing_suggestions_for(&mut self, edit: &CellEdit, cx: &App) -> Vec<(&'static str, &'static str)> {
        let saved = self.editing.replace(CellEdit {
            path: edit.path.clone(),
            column: edit.column,
            setting: edit.setting,
            input: edit.input.clone(),
            highlighted: None,
            _subscriptions: vec![],
        });
        let out = self.suggestions(cx);
        self.editing = saved;
        out
    }

    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            self.focus_handle.focus(window, cx);
            cx.notify();
        }
    }

    fn pick_suggestion(&mut self, name: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.editing.take() else { return };
        self.commit_key(edit.path, name, window, cx);
        self.focus_handle.focus(window, cx);
    }

    // ------------------------------------------------------------------
    // Keyboard

    fn select_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(edit) = self.editing.as_ref().filter(|e| e.column == Column::Key) {
            let count = self.suggestions(cx).len();
            if count > 0 {
                let next = match edit.highlighted {
                    None if delta > 0 => 0,
                    None => count - 1,
                    Some(ix) => (ix as isize + delta).rem_euclid(count as isize) as usize,
                };
                if let Some(edit) = self.editing.as_mut() {
                    edit.highlighted = Some(next);
                }
                cx.notify();
            }
            return;
        }
        let rows = self.rows(cx);
        let file_rows: Vec<&Row> = rows.iter().filter(|r| r.setting.is_none()).collect();
        if file_rows.is_empty() {
            return;
        }
        let current = self.selected.as_ref().and_then(|p| file_rows.iter().position(|r| &r.path == p));
        let next = match current {
            None => 0,
            Some(ix) => (ix as isize + delta).clamp(0, file_rows.len() as isize - 1) as usize,
        };
        self.selected = Some(file_rows[next].path.clone());
        self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn toggle_selected(&mut self, expand: bool, cx: &mut Context<Self>) {
        let Some(path) = self.selected.clone() else { return };
        let is_container = self.node(&path).is_some_and(|n| n.value.kind().is_container());
        let is_expanded = self.root.as_ref().ok().is_some_and(|root| self.expanded.contains(&path_id(root, &path)));
        if is_container && expand != is_expanded {
            self.expand(&path, expand);
        } else if !expand && !path.is_empty() {
            self.selected = Some(path[..path.len() - 1].to_vec());
        }
        cx.notify();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.commit_edit(window, cx);
            self.focus_handle.focus(window, cx);
            return;
        }
        let Some(path) = self.selected.clone() else { return };
        let in_dict = path.split_last().and_then(|(_, parent)| self.node(parent)).is_some_and(|p| matches!(p.value, Value::Dict(_)));
        let column = if in_dict { Column::Key } else { Column::Value };
        self.begin_edit(path, column, None, window, cx);
    }

    // ------------------------------------------------------------------
    // The debug control channel

    /// The visible rows as `depth|key|type|value` lines (raw key names
    /// after the shown key, when they differ).
    pub fn summary(&self, cx: &App) -> serde_json::Value {
        let raw = raw_keys();
        // The text as it is now, even if this hasn't drawn since it changed.
        let text = self.text.read(cx).text().to_string();
        let parsed = plist::parse_xml(&text).map_err(|err| format!("{err:#}"));
        let rows = parsed.as_ref().map(|root| self.build_rows_from(root)).unwrap_or_default();
        let lines: Vec<String> = rows
            .iter()
            .map(|row| {
                let label = self.key_label(row, raw);
                let mut key = label.clone();
                if let Some(raw_key) = &row.key
                    && *raw_key != label
                {
                    key = format!("{label} ({raw_key})");
                }
                let setting = if row.setting.is_some() { " [build setting]" } else { "" };
                format!("{}|{key}|{}|{}{setting}", row.depth, row.kind.label(), Self::value_label(row, raw))
            })
            .collect();
        serde_json::json!({
            "rows": lines,
            "error": self.error,
            "parse_error": parsed.as_ref().err(),
            "raw": raw,
            "selected": self.selected,
            "editing": self.editing.as_ref().map(|e| e.input.read(cx).value().to_string()),
            "suggestions": self.suggestions(cx).iter().take(5).map(|(name, _)| name.to_string()).collect::<Vec<_>>(),
        })
    }

    /// The path of a `/`-separated key path (`UIBackgroundModes/0`, `New%20item`).
    fn find(&mut self, key_path: &str, cx: &App) -> Result<Vec<usize>, String> {
        self.refresh(cx);
        let root = self.root.as_ref().map_err(|e| e.clone())?;
        let mut path = vec![];
        let mut node = root;
        // `%20` stands for a space, which separates the command's arguments.
        for part in key_path.split('/').filter(|p| !p.is_empty()).map(|p| p.replace("%20", " ")) {
            let part = part.as_str();
            let ix = match &node.value {
                Value::Dict(entries) => entries.iter().position(|e| e.key == part),
                Value::Array(items) => part.parse::<usize>().ok().filter(|&i| i < items.len()),
                _ => None,
            }
            .ok_or_else(|| format!("no {part} in {key_path}"))?;
            path.push(ix);
            node = node.child(ix).expect("found").1;
        }
        Ok(path)
    }

    /// Runs a debug-channel command as the matching click or key would:
    /// `add <path>`, `remove <path>`, `key <path> <text>`, `value <path>
    /// <text>`, `type <path> <Kind>`, `move <path> up|down`, `expand
    /// <path>`, `collapse <path>`, `type-key <path> <text>` (start editing a
    /// key and type, leaving suggestions open), `pick <n>` (a suggestion),
    /// `setting <key> <value>`, `remove-setting <key>`.
    pub fn command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let (verb, rest) = command.split_once(' ').unwrap_or((command, ""));
        let (path_text, arg) = rest.split_once(' ').unwrap_or((rest, ""));
        let path_text = if path_text == "/" { "" } else { path_text };
        self.error = None;
        match verb {
            "add" => {
                let path = self.find(path_text, cx)?;
                self.add_item(Some(path), window, cx);
            }
            "remove" => {
                let path = self.find(path_text, cx)?;
                self.remove(path, window, cx);
            }
            "key" | "type-key" => {
                let path = self.find(path_text, cx)?;
                self.begin_edit(path, Column::Key, None, window, cx);
                let input = self.editing.as_ref().map(|e| e.input.clone()).ok_or("not editable")?;
                input.update(cx, |state, cx| state.set_value(arg.to_string(), window, cx));
                if verb == "key" {
                    self.commit_edit(window, cx);
                }
            }
            "value" => {
                let path = self.find(path_text, cx)?;
                let kind = self.node(&path).map(|n| n.value.kind());
                if kind == Some(Kind::Boolean) {
                    let value = plist::parse_bool(arg).ok_or("not YES or NO")?;
                    self.set_value(path, Value::Bool(value), window, cx);
                } else {
                    self.begin_edit(path, Column::Value, None, window, cx);
                    let input = self.editing.as_ref().map(|e| e.input.clone()).ok_or("not editable")?;
                    input.update(cx, |state, cx| state.set_value(arg.to_string(), window, cx));
                    self.commit_edit(window, cx);
                }
            }
            "type" => {
                let path = self.find(path_text, cx)?;
                let kind = Kind::ALL.into_iter().find(|k| k.label() == arg).ok_or(format!("no type {arg}"))?;
                self.set_type(path, kind, window, cx);
            }
            "move" => {
                self.selected = Some(self.find(path_text, cx)?);
                self.move_selected(if arg == "up" { -1 } else { 1 }, window, cx);
            }
            "expand" | "collapse" => {
                let path = self.find(path_text, cx)?;
                self.expand(&path, verb == "expand");
                cx.notify();
            }
            "pick" => {
                let ix: usize = path_text.parse().map_err(|_| "usage: pick <n>")?;
                let name = self.suggestions(cx).get(ix).map(|(n, _)| *n).ok_or("no such suggestion")?;
                self.pick_suggestion(name, window, cx);
            }
            "setting" | "remove-setting" => {
                let ix = self.settings.iter().position(|s| s.key == path_text).ok_or(format!("no build setting row {path_text}"))?;
                if verb == "setting" {
                    self.commit_setting(ix, arg, cx);
                } else {
                    self.remove_setting(ix, cx);
                }
            }
            other => return Err(format!("unknown plist command {other}")),
        }
        match &self.error {
            Some(err) => Err(err.clone()),
            None => Ok(()),
        }
    }

    // ------------------------------------------------------------------
    // Rendering

    fn type_menu(&self, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let id = SharedString::from(format!("plist-type-{}", row_key(row)));
        let label = row.kind.label();
        if row.setting.is_some() {
            return div().text_sm().text_color(cx.theme().muted_foreground).child(label).into_any_element();
        }
        let path = row.path.clone();
        let current = row.kind;
        let editor = cx.weak_entity();
        let root = row.path.is_empty();
        Button::new(id)
            .label(label)
            .xsmall()
            .ghost()
            .dropdown_menu(move |mut menu: PopupMenu, _, _| {
                for (ix, kind) in Kind::ALL.into_iter().enumerate() {
                    if ix == 2 {
                        menu = menu.separator();
                    }
                    let (editor, path) = (editor.clone(), path.clone());
                    menu = menu.item(
                        PopupMenuItem::new(kind.label()).checked(kind == current).disabled(root && !kind.is_container()).on_click(
                            move |_, window, cx| {
                                editor.update(cx, |this, cx| this.set_type(path.clone(), kind, window, cx)).ok();
                            },
                        ),
                    );
                }
                menu
            })
            .into_any_element()
    }

    fn value_cell(&self, row: &Row, raw: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if let Some(edit) = self.editing.as_ref().filter(|e| e.column == Column::Value && e.path == row.path && e.setting == row.setting) {
            return div().flex_1().min_w_0().child(Input::new(&edit.input).xsmall()).into_any_element();
        }
        let label = Self::value_label(row, raw);
        if row.kind.is_container() {
            return div().flex_1().min_w_0().truncate().text_color(theme.muted_foreground).child(label).into_any_element();
        }
        let editor = cx.weak_entity();
        let id = row_key(row);
        // Booleans and values with a fixed set of choices get a menu.
        let choices: Vec<(String, String)> = match &row.value {
            Value::Bool(_) => vec![("YES".into(), "YES".into()), ("NO".into(), "NO".into())],
            Value::String(_) => row
                .resolved
                .choices()
                .iter()
                .map(|c| (c.value.to_string(), if raw { c.value.to_string() } else { c.label.to_string() }))
                .collect(),
            _ => vec![],
        };
        let text = div().flex_1().min_w_0().truncate().child(label.clone());
        if choices.is_empty() {
            return text.into_any_element();
        }
        let (path, setting, is_bool) = (row.path.clone(), row.setting, matches!(row.value, Value::Bool(_)));
        let current = match &row.value {
            Value::Bool(b) => if *b { "YES" } else { "NO" }.to_string(),
            Value::String(s) => s.clone(),
            _ => String::new(),
        };
        h_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(text)
            .child(
                Button::new(SharedString::from(format!("plist-choices-{id}")))
                    .icon(IconName::ChevronsUpDown)
                    .xsmall()
                    .ghost()
                    .dropdown_menu(move |mut menu: PopupMenu, _, _| {
                        for (value, label) in &choices {
                            let (editor, path, value) = (editor.clone(), path.clone(), value.clone());
                            menu = menu.item(PopupMenuItem::new(label.clone()).checked(*value == current).on_click(move |_, window, cx| {
                                editor
                                    .update(cx, |this, cx| match setting {
                                        Some(ix) => this.commit_setting(ix, &value, cx),
                                        None if is_bool => this.set_value(path.clone(), Value::Bool(value == "YES"), window, cx),
                                        None => this.set_value(path.clone(), Value::String(value.clone()), window, cx),
                                    })
                                    .ok();
                            }));
                        }
                        menu
                    }),
            )
            .into_any_element()
    }

    fn key_cell(&self, row: &Row, raw: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let label = self.key_label(row, raw);
        let editing = self.editing.as_ref().filter(|e| e.column == Column::Key && e.path == row.path && row.setting.is_none());
        let disclosure = row.kind.is_container() && row.setting.is_none();
        let path = row.path.clone();
        let chevron = div()
            .id(SharedString::from(format!("plist-disclose-{}", row_key(row))))
            .w(px(14.))
            .flex_none()
            .when(disclosure, |el| {
                el.child(Icon::new(if row.expanded { IconName::ChevronDown } else { IconName::ChevronRight }).xsmall())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let expanded = this.root.as_ref().ok().is_some_and(|root| this.expanded.contains(&path_id(root, &path)));
                        this.expand(&path, !expanded);
                        cx.notify();
                    }))
            });
        let mut cell = h_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .gap_1()
            .pl(px(4. + row.depth as f32 * INDENT))
            .child(chevron);
        match editing {
            Some(edit) => {
                let suggestions = self.suggestions(cx);
                let highlighted = edit.highlighted;
                let list = (!suggestions.is_empty()).then(|| {
                    let editor = cx.weak_entity();
                    let item_theme = theme.clone();
                    let items = suggestions.into_iter().take(12).enumerate().map(move |(ix, (name, label))| {
                        let editor = editor.clone();
                        let theme = item_theme.clone();
                        let shown = if label.is_empty() || raw { name } else { label };
                        div()
                            .id(("plist-suggestion", ix))
                            .px_2()
                            .py_0p5()
                            .rounded(theme.radius)
                            .truncate()
                            .when(highlighted == Some(ix), |el| el.bg(theme.accent).text_color(theme.accent_foreground))
                            .hover(|el| el.bg(theme.secondary_hover))
                            .child(shown)
                            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                cx.stop_propagation();
                                editor.update(cx, |this, cx| this.pick_suggestion(name, window, cx)).ok();
                            })
                    });
                    deferred(
                        anchored().snap_to_window_with_margin(px(8.)).child(
                            v_flex()
                                .id("plist-suggestions")
                                // Below the field being typed in.
                                .mt(px(ROW_HEIGHT + 2.))
                                .w(px(320.))
                                .max_h(px(260.))
                                .overflow_y_scroll()
                                .p_1()
                                .text_sm()
                                .bg(theme.popover)
                                .border_1()
                                .border_color(theme.border)
                                .rounded(theme.radius)
                                .shadow_md()
                                .children(items),
                        ),
                    )
                    .with_priority(1)
                });
                cell = cell.child(v_flex().flex_1().min_w_0().child(Input::new(&edit.input).xsmall()).children(list));
            }
            None => {
                cell = cell.child(div().flex_1().min_w_0().truncate().when(row.setting.is_some(), |el| el.italic()).child(label));
            }
        }
        // + and − at the end of the key column, as Xcode shows them.
        let (add_path, remove_path, setting) = (row.path.clone(), row.path.clone(), row.setting);
        let removable = setting.is_some() || !row.path.is_empty();
        let id = row_key(row);
        cell.child(
            h_flex()
                .flex_none()
                .invisible()
                .group_hover("plist-row", |s| s.visible())
                .when(setting.is_none(), |el| {
                    el.child(
                        Button::new(SharedString::from(format!("plist-add-{id}")))
                            .icon(IconName::Plus)
                            .xsmall()
                            .ghost()
                            .tooltip("Add Item")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.add_item(Some(add_path.clone()), window, cx);
                            })),
                    )
                })
                .when(removable, |el| {
                    el.child(
                        Button::new(SharedString::from(format!("plist-remove-{id}")))
                            .icon(IconName::Minus)
                            .xsmall()
                            .ghost()
                            .tooltip("Delete")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                match setting {
                                    Some(ix) => this.remove_setting(ix, cx),
                                    None => this.remove(remove_path.clone(), window, cx),
                                }
                            })),
                    )
                }),
        )
        .into_any_element()
    }

    fn render_rows(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let rows = self.rows(cx);
        let raw = raw_keys();
        let theme = cx.theme().clone();
        let mut out = vec![];
        for row in &rows[range.start.min(rows.len())..range.end.min(rows.len())] {
            let selected = row.setting.is_none() && self.selected.as_ref() == Some(&row.path);
            let key = self.key_cell(row, raw, cx);
            let ty = self.type_menu(row, cx);
            let value = self.value_cell(row, raw, cx);
            let (path, setting) = (row.path.clone(), row.setting);
            let (key_path, value_path) = (row.path.clone(), row.path.clone());
            let in_dict = row.key.is_some();
            let dragged = DraggedRow { path: row.path.clone(), label: self.key_label(row, raw).into() };
            let drop_path = row.path.clone();
            let menu_path = row.path.clone();
            let editor = cx.weak_entity();
            let (is_root, is_container) = (row.path.is_empty() && setting.is_none(), row.kind.is_container());
            let element = h_flex()
                .id(SharedString::from(format!("plist-row-{}", row_key(row))))
                .group("plist-row")
                .h(px(ROW_HEIGHT))
                .w_full()
                .pr_2()
                .text_sm()
                .when(selected, |el| el.bg(theme.accent.opacity(0.6)))
                .when(!selected, |el| el.hover(|el| el.bg(theme.secondary_hover)))
                .child(
                    div()
                        .id(SharedString::from(format!("plist-key-{}", row_key(row))))
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(key)
                        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                            // Double-clicking a key edits it.
                            if event.click_count() >= 2 && in_dict && setting.is_none() {
                                cx.stop_propagation();
                                this.begin_edit(key_path.clone(), Column::Key, None, window, cx);
                            }
                        })),
                )
                .child(div().w(px(TYPE_WIDTH)).flex_none().child(ty))
                .child(
                    div()
                        .id(SharedString::from(format!("plist-value-{}", row_key(row))))
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .items_center()
                        .pl_2()
                        .child(value)
                        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                            if event.click_count() >= 2 {
                                cx.stop_propagation();
                                this.begin_edit(value_path.clone(), Column::Value, setting, window, cx);
                            }
                        })),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    if setting.is_none() {
                        this.selected = Some(path.clone());
                    }
                    if this.editing.is_none() {
                        this.focus_handle.focus(window, cx);
                    }
                    cx.notify();
                }))
                .when(setting.is_none() && !is_root, |el| {
                    el.on_drag(dragged, |row, _, _, cx| cx.new(|_| row.clone()))
                        .drag_over::<DraggedRow>(|style, _, _, cx| style.bg(cx.theme().drop_target))
                        .on_drop(cx.listener(move |this, dragged: &DraggedRow, window, cx| {
                            let (Some((&to, to_parent)), Some((_, from_parent))) = (drop_path.split_last(), dragged.path.split_last()) else {
                                return;
                            };
                            if to_parent == from_parent {
                                this.move_to(dragged.path.clone(), to, window, cx);
                            }
                        }))
                });
            out.push(if setting.is_none() {
                element
                    .context_menu(move |menu, _, _| row_menu(menu, editor.clone(), menu_path.clone(), is_root, is_container))
                    .into_any_element()
            } else {
                element.into_any_element()
            });
        }
        out
    }
}

fn row_key(row: &Row) -> String {
    match row.setting {
        Some(ix) => format!("s{ix}"),
        None => row.path.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("-"),
    }
}

/// A build setting's value as the row shows it: Booleans as YES/NO,
/// space-separated lists as text.
fn setting_value(setting: &SettingRow, resolved: &Resolved) -> Value {
    match (&setting.value, resolved.kind) {
        (None, _) => Value::String("Multiple values".into()),
        (Some(v), Some(Kind::Boolean)) => plist::parse_bool(v).map_or(Value::String(v.clone()), Value::Bool),
        (Some(v), _) => Value::String(v.clone()),
    }
}

fn row_menu(menu: PopupMenu, editor: WeakEntity<PlistEditor>, path: Vec<usize>, is_root: bool, is_container: bool) -> PopupMenu {
    let (e1, e2, e3, e4, e5) = (editor.clone(), editor.clone(), editor.clone(), editor.clone(), editor.clone());
    let (p1, p2, p3, p4) = (path.clone(), path.clone(), path.clone(), path.clone());
    let mut menu = menu
        .item(PopupMenuItem::new("Add Item").on_click(move |_, window, cx| {
            e1.update(cx, |this, cx| this.add_item(Some(p1.clone()), window, cx)).ok();
        }))
        .item(PopupMenuItem::new("Delete").disabled(is_root).on_click(move |_, window, cx| {
            e2.update(cx, |this, cx| this.remove(p2.clone(), window, cx)).ok();
        }))
        .separator()
        .item(PopupMenuItem::new("Move Up").disabled(is_root).on_click(move |_, window, cx| {
            e3.update(cx, |this, cx| {
                this.selected = Some(p3.clone());
                this.move_selected(-1, window, cx)
            })
            .ok();
        }))
        .item(PopupMenuItem::new("Move Down").disabled(is_root).on_click(move |_, window, cx| {
            e4.update(cx, |this, cx| {
                this.selected = Some(p4.clone());
                this.move_selected(1, window, cx)
            })
            .ok();
        }))
        .separator();
    for kind in Kind::ALL {
        let (editor, path) = (editor.clone(), path.clone());
        menu = menu.item(PopupMenuItem::new(format!("Value Type: {}", kind.label())).disabled(is_root && !kind.is_container()).on_click(
            move |_, window, cx| {
                editor.update(cx, |this, cx| this.set_type(path.clone(), kind, window, cx)).ok();
            },
        ));
    }
    let _ = is_container;
    menu.separator().item(PopupMenuItem::new("Raw Keys and Values").checked(raw_keys()).on_click(move |_, _, cx| {
        RAW_KEYS.fetch_xor(true, Ordering::Relaxed);
        e5.update(cx, |_, cx| cx.notify()).ok();
    }))
}

impl Render for PlistEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let rows = self.rows(cx);
        let parse_error = self.root.as_ref().err().cloned().filter(|e| !e.is_empty());
        let header = h_flex()
            .h(px(24.))
            .flex_none()
            .pr_2()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.muted_foreground)
            .border_b_1()
            .border_color(theme.border)
            .child(div().flex_1().pl(px(22.)).child("Key"))
            .child(div().w(px(TYPE_WIDTH)).flex_none().pl_2().child("Type"))
            .child(div().flex_1().pl_2().child("Value"));
        // Undo and Redo go to the text editor, which is rendered (empty) so
        // it can take them. The guard stops a handler it doesn't stop from
        // coming back here.
        let text_focus = self.text.read(cx).focus_handle(cx);
        let forwarding = Rc::new(Cell::new(false));
        let forward = |action: Box<dyn Action>, handle: FocusHandle, guard: Rc<Cell<bool>>| {
            move |window: &mut Window, cx: &mut App| {
                if !guard.replace(true) {
                    handle.dispatch_action(action.as_ref(), window, cx);
                    guard.set(false);
                }
            }
        };
        let undo = forward(Box::new(Undo), text_focus.clone(), forwarding.clone());
        let redo = forward(Box::new(Redo), text_focus, forwarding);
        let body = match parse_error {
            Some(err) => v_flex()
                .p_4()
                .gap_2()
                .text_sm()
                .child(div().text_color(theme.danger).child("The property list can’t be read."))
                .child(div().text_color(theme.muted_foreground).child(err))
                .child(
                    Button::new("plist-open-source")
                        .label("Open As Source Code")
                        .small()
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(PlistEditorEvent::OpenAsSource))),
                )
                .into_any_element(),
            None => uniform_list("plist-rows", rows.len(), cx.processor(Self::render_rows))
                .track_scroll(&self.scroll)
                .flex_1()
                .into_any_element(),
        };
        let _ = window;
        v_flex()
            .id("plist-editor")
            .key_context("PlistEditor")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .on_action(move |_: &Undo, window, cx| undo(window, cx))
            .on_action(move |_: &Redo, window, cx| redo(window, cx))
            .on_action(cx.listener(|this, _: &AddPlistItem, window, cx| {
                let at = this.selected.clone();
                this.add_item(at, window, cx)
            }))
            .on_action(cx.listener(|this, _: &DeletePlistItem, window, cx| {
                if let Some(path) = this.selected.clone() {
                    this.remove(path, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &MoveLineUp, window, cx| this.move_selected(-1, window, cx)))
            .on_action(cx.listener(|this, _: &MoveLineDown, window, cx| this.move_selected(1, window, cx)))
            .on_action(cx.listener(|this, _: &PickerSelectNext, _, cx| this.select_step(1, cx)))
            .on_action(cx.listener(|this, _: &PickerSelectPrevious, _, cx| this.select_step(-1, cx)))
            .on_action(cx.listener(|this, _: &PickerConfirm, window, cx| this.confirm(window, cx)))
            .on_action(cx.listener(|this, _: &PickerDismiss, window, cx| this.cancel_edit(window, cx)))
            .on_action(cx.listener(|this, _: &PlistExpand, _, cx| this.toggle_selected(true, cx)))
            .on_action(cx.listener(|this, _: &PlistCollapse, _, cx| this.toggle_selected(false, cx)))
            .on_action(cx.listener(|this, _: &ToggleRawKeysAndValues, _, cx| {
                RAW_KEYS.fetch_xor(true, Ordering::Relaxed);
                this.rows_stale = true;
                cx.notify();
            }))
            .on_action(cx.listener(|_, _: &OpenAsSourceCode, _, cx| cx.emit(PlistEditorEvent::OpenAsSource)))
            .on_action(cx.listener(|this, _: &SetPlistTypeArray, window, cx| this.set_selected_type(Kind::Array, window, cx)))
            .on_action(cx.listener(|this, _: &SetPlistTypeDictionary, window, cx| this.set_selected_type(Kind::Dictionary, window, cx)))
            .on_action(cx.listener(|this, _: &SetPlistTypeBoolean, window, cx| this.set_selected_type(Kind::Boolean, window, cx)))
            .on_action(cx.listener(|this, _: &SetPlistTypeData, window, cx| this.set_selected_type(Kind::Data, window, cx)))
            .on_action(cx.listener(|this, _: &SetPlistTypeDate, window, cx| this.set_selected_type(Kind::Date, window, cx)))
            .on_action(cx.listener(|this, _: &SetPlistTypeNumber, window, cx| this.set_selected_type(Kind::Number, window, cx)))
            .on_action(cx.listener(|this, _: &SetPlistTypeString, window, cx| this.set_selected_type(Kind::String, window, cx)))
            .child(header)
            .when_some(self.error.clone(), |el, err| {
                el.child(div().px_3().py_1().text_sm().bg(theme.danger.opacity(0.12)).text_color(theme.danger).child(err))
            })
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
            .child(div().size_0().overflow_hidden().child(Editor::new(&self.text)))
    }
}

impl PlistEditor {
    fn set_selected_type(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.selected.clone() {
            self.set_type(path, kind, window, cx);
        }
    }
}

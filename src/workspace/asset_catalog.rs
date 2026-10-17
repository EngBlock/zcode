//! The asset catalog editor: an `.xcassets` folder as a tab, as Xcode
//! shows it. The outline of sets and folders on the left, the selected
//! set's wells in the middle (a thumbnail drawn from each slot's file, a
//! swatch from each color's components; a folder shows its sets as tiles),
//! and the attributes inspector on the right.
//!
//! Edits (dropping images into wells or onto the outline, new and removed
//! image and color sets, renaming, a color's components) are file
//! operations (`xcassets::edit`) applied in the background one at a time,
//! with the project's edit lock, and undone with ⌘Z. Catalogs are folders
//! Xcode references as one file, so `project.pbxproj` never changes; the
//! workspace rescans afterwards so new files show up (synchronized folders
//! build them as they are).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{Value, json};
use zcode_editor::{Redo, Undo};

use crate::actions::{DeletePlistItem, PickerSelectNext, PickerSelectPrevious};
use crate::xcassets::color::{self, ColorValue, InputMethod};
use crate::xcassets::edit::{self, Edit, Op, Stash};
use crate::xcassets::thumbnails::{self, ThumbnailCache};
use crate::xcassets::{self, AssetSet, Catalog, Node, NodeKind, SetKind, Slot, slots};

/// A well's side, in points.
const WELL: f32 = 88.;
/// Tiles per row when a folder is shown.
const TILE_COLUMNS: usize = 5;

/// Where rows, wells, tiles and fields were last drawn (window points),
/// for the control channel's clicks and drops.
type Places = Rc<RefCell<BTreeMap<String, Bounds<Pixels>>>>;

/// Records the bounds of the element it's put in (which must be relative).
fn place(places: &Places, key: String) -> impl IntoElement {
    let places = places.clone();
    canvas(move |bounds, _, _| {
        places.borrow_mut().insert(key, bounds);
    }, |_, _, _, _| {})
    .absolute()
    .size_full()
}

pub enum AssetCatalogEvent {
    /// Files were written: the project's file list is stale.
    FilesChanged,
    Error(String),
}

impl EventEmitter<AssetCatalogEvent> for AssetCatalogView {}

/// A row of the outline.
#[derive(Clone, Debug)]
struct OutlineRow {
    path: PathBuf,
    name: String,
    depth: usize,
    folder: bool,
    expanded: bool,
    kind: Option<SetKind>,
    swatch: Option<[f32; 4]>,
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Do,
    Undo,
    Redo,
}

pub struct AssetCatalogView {
    focus_handle: FocusHandle,
    pub root: PathBuf,
    pub catalog: Option<Arc<Catalog>>,
    pub load_error: Option<String>,
    /// The set or folder shown (the catalog itself at first).
    pub selected: PathBuf,
    /// The selected well: its entry's index in the set.
    pub slot: Option<usize>,
    expanded: HashSet<PathBuf>,
    rows: Vec<OutlineRow>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    /// An edit is being applied.
    pub busy: bool,
    /// The last edit's error.
    pub error: Option<String>,
    stash: Stash,
    lock: Arc<parking_lot::Mutex<()>>,
    name: Entity<InputState>,
    name_for: Option<PathBuf>,
    /// What the name field was filled with: only what's typed over it is
    /// committed.
    name_shown: String,
    /// A color's component fields: its key, field and what it was filled
    /// with.
    components: Vec<(String, Entity<InputState>, String)>,
    component_subscriptions: Vec<Subscription>,
    components_for: Option<(PathBuf, usize, String)>,
    outline_scroll: UniformListScrollHandle,
    places: Places,
    /// Reloads since the tab opened (for tests).
    pub generation: u64,
    _subscriptions: Vec<Subscription>,
    _load: Option<Task<()>>,
}

impl Focusable for AssetCatalogView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl AssetCatalogView {
    pub fn new(
        root: PathBuf,
        select: Option<PathBuf>,
        stash: PathBuf,
        lock: Arc<parking_lot::Mutex<()>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Name"));
        let subscription = cx.subscribe_in(&name, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.commit_name(window, cx);
            }
        });
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            selected: select.clone().unwrap_or_else(|| root.clone()),
            root,
            catalog: None,
            load_error: None,
            slot: None,
            expanded: HashSet::new(),
            rows: vec![],
            undo: vec![],
            redo: vec![],
            busy: false,
            error: None,
            stash: Stash::new(stash),
            lock,
            name,
            name_for: None,
            name_shown: String::new(),
            components: vec![],
            component_subscriptions: vec![],
            components_for: None,
            outline_scroll: UniformListScrollHandle::new(),
            places: Places::default(),
            generation: 0,
            _subscriptions: vec![subscription],
            _load: None,
        };
        this.reload(window, cx);
        this
    }

    // ---------------------------------------------------------------------
    // Loading and selection

    /// Reads the catalog again (in the background), keeping the selection.
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        self._load = Some(cx.spawn_in(window, async move |this, cx| {
            let loaded = cx.background_spawn(async move { Catalog::load(&root) }).await;
            this.update_in(cx, |this, window, cx| {
                match loaded {
                    Ok(catalog) => {
                        this.catalog = Some(Arc::new(catalog));
                        this.load_error = None;
                    }
                    Err(err) => this.load_error = Some(err.to_string()),
                }
                this.generation += 1;
                let selected = this.selected.clone();
                this.show_path(selected, window, cx);
            })
            .ok();
        }));
    }

    fn node(&self, path: &Path) -> Option<&Node> {
        self.catalog.as_ref()?.find(path)
    }

    pub fn selected_node(&self) -> Option<&Node> {
        self.node(&self.selected)
    }

    pub fn selected_set(&self) -> Option<&AssetSet> {
        self.selected_node().and_then(Node::set)
    }

    fn selected_slots(&self) -> Vec<Slot> {
        self.selected_set().map(AssetSet::slots).unwrap_or_default()
    }

    /// Shows a set or folder (the catalog when it's gone), its folders
    /// expanded.
    pub fn select_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_pending(window, cx);
        self.show_path(path, window, cx);
    }

    /// Shows a set or folder without committing the fields (after a reload).
    fn show_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let path = if self.catalog.is_none() || self.node(&path).is_some() {
            path
        } else {
            // Removed or renamed: the nearest folder still there.
            path.ancestors().find(|p| self.node(p).is_some()).map(Path::to_path_buf).unwrap_or_else(|| self.root.clone())
        };
        if path != self.selected {
            self.slot = None;
        }
        for ancestor in path.ancestors().skip(1) {
            if !ancestor.starts_with(&self.root) || ancestor == self.root {
                break;
            }
            self.expanded.insert(ancestor.to_path_buf());
        }
        self.selected = path;
        let count = self.selected_set().map(|s| s.entries().len()).unwrap_or(0);
        if self.slot.is_some_and(|ix| ix >= count) {
            self.slot = None;
        }
        self.rebuild_rows();
        if let Some(ix) = self.rows.iter().position(|r| r.path == self.selected) {
            self.outline_scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        }
        self.sync_inputs(window, cx);
        cx.notify();
    }

    pub fn select_slot(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_pending(window, cx);
        self.slot = index;
        self.sync_inputs(window, cx);
        cx.notify();
    }

    /// Selects by path, a path in the catalog (`Icons/Star`), or a name.
    pub fn select_named(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let catalog = self.catalog.clone().ok_or("the catalog isn't loaded")?;
        let found = if query.is_empty() || query == "/" {
            Some(self.root.clone())
        } else if Path::new(query).is_absolute() {
            catalog.find(Path::new(query)).map(|n| n.path.clone())
        } else {
            let mut all = vec![&catalog.root];
            all.extend(Self::all_nodes(&catalog.root));
            let relative = |n: &Node| {
                let rel = n.path.strip_prefix(&self.root).unwrap_or(&n.path);
                rel.components().map(|c| xcassets::display_name(Path::new(c.as_os_str()))).collect::<Vec<_>>().join("/")
            };
            all.iter().find(|n| relative(n) == query).or_else(|| all.iter().find(|n| n.name == query)).map(|n| n.path.clone())
        };
        let path = found.ok_or_else(|| format!("no asset {query}"))?;
        self.select_path(path, window, cx);
        Ok(())
    }

    fn all_nodes(node: &Node) -> Vec<&Node> {
        let mut out = vec![];
        for child in &node.children {
            out.push(child);
            out.extend(Self::all_nodes(child));
        }
        out
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        let Some(catalog) = self.catalog.clone() else { return };
        fn add(rows: &mut Vec<OutlineRow>, node: &Node, depth: usize, expanded: &HashSet<PathBuf>) {
            for child in &node.children {
                let open = expanded.contains(&child.path);
                let set = child.set();
                rows.push(OutlineRow {
                    path: child.path.clone(),
                    name: child.name.clone(),
                    depth,
                    folder: child.is_folder(),
                    expanded: open,
                    kind: set.map(|s| s.kind),
                    swatch: set
                        .filter(|s| s.kind == SetKind::Color)
                        .and_then(|s| s.entries().first())
                        .map(|e| ColorValue::of(e).rgba()),
                });
                if child.is_folder() && open {
                    add(rows, child, depth + 1, expanded);
                }
            }
        }
        add(&mut self.rows, &catalog.root, 0, &self.expanded);
    }

    pub fn toggle_folder(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_path_buf());
        }
        self.rebuild_rows();
        cx.notify();
    }

    /// ↑ / ↓ in the outline.
    fn step_selection(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self.rows.iter().position(|r| r.path == self.selected);
        let next = match (ix, forward) {
            (None, _) => 0,
            (Some(ix), true) => (ix + 1).min(self.rows.len().saturating_sub(1)),
            (Some(ix), false) => ix.saturating_sub(1),
        };
        if let Some(row) = self.rows.get(next) {
            let path = row.path.clone();
            self.select_path(path, window, cx);
        }
    }

    // ---------------------------------------------------------------------
    // The inspector's fields

    /// Fills the name and color fields from the selection.
    fn sync_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.selected_node().filter(|n| n.path != self.root).map(|n| n.name.clone());
        if self.name_for.as_ref() != Some(&self.selected) || name.as_deref() != Some(self.name.read(cx).value().as_ref()) {
            self.name_for = Some(self.selected.clone());
            let value = name.unwrap_or_default();
            self.name_shown = value.clone();
            self.name.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        let color = self.slot.and_then(|ix| {
            let set = self.selected_set().filter(|s| s.kind == SetKind::Color)?;
            Some((ix, set.entries().get(ix)?.clone()))
        });
        let Some((ix, entry)) = color else {
            self.components.clear();
            self.component_subscriptions.clear();
            self.components_for = None;
            return;
        };
        let value = ColorValue::of(&entry);
        let space = match &value {
            ColorValue::Components { space, .. } => space.clone(),
            _ => "srgb".into(),
        };
        let key = (self.selected.clone(), ix, entry.to_string());
        if self.components_for.as_ref() == Some(&key) {
            return;
        }
        self.components_for = Some(key);
        let method = value.input_method();
        self.component_subscriptions.clear();
        self.components = color::component_keys(&space)
            .iter()
            .map(|component| {
                let current = value.component(component).unwrap_or(1.);
                let text = if *component == "alpha" { InputMethod::Float.format(current) } else { method.format(current) };
                let input = cx.new(|cx| InputState::new(window, cx).default_value(text.clone()));
                let (set, component) = (self.selected.clone(), component.to_string());
                let key = component.clone();
                let subscription = cx.subscribe_in(&input, window, move |this, input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        let text = input.read(cx).value().to_string();
                        this.commit_component(&set, ix, &key, &text, window, cx);
                    }
                });
                self.component_subscriptions.push(subscription);
                (component, input, text)
            })
            .collect();
    }

    /// A rename or component typed but not committed yet, before the
    /// selection moves on.
    fn commit_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_name(window, cx);
        if let Some((set, ix, _)) = self.components_for.clone() {
            for (key, input, _) in self.components.clone() {
                let text = input.read(cx).value().to_string();
                self.commit_component(&set, ix, &key, &text, window, cx);
            }
        }
    }

    fn commit_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.name_for.clone() else { return };
        let text = self.name.read(cx).value().trim().to_string();
        let current = self.node(&path).map(|n| n.name.clone());
        if text == self.name_shown.trim() || path == self.root || current.is_none() || current.as_deref() == Some(text.as_str()) || text.is_empty() {
            return;
        }
        self.name_shown = text.clone();
        if let Err(err) = self.rename(&path, &text, window, cx) {
            self.report(err, cx);
            let value = current.unwrap_or_default();
            self.name.update(cx, |input, cx| input.set_value(value, window, cx));
        }
    }

    fn commit_component(&mut self, set: &Path, ix: usize, key: &str, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        // Only what was typed over the field's value: a field left alone
        // says what the file said when it was filled.
        let shown = self.components_for.as_ref().is_some_and(|(s, i, _)| s == set && *i == ix);
        match self.components.iter_mut().find(|(k, _, _)| k == key) {
            Some((_, _, filled)) if shown => {
                if filled.trim() == text.trim() {
                    return;
                }
                *filled = text.to_string();
            }
            _ => return,
        }
        let Some(entry) = self.node(set).and_then(Node::set).and_then(|s| s.entries().get(ix)).cloned() else { return };
        let value = ColorValue::of(&entry);
        let method = if key == "alpha" { InputMethod::Float } else { value.input_method() };
        let Some(parsed) = method.parse(text) else { return };
        if value.component(key).is_some_and(|v| method.format(v) == method.format(parsed)) {
            return;
        }
        if let Err(err) = self.set_color_component_at(set, ix, key, parsed, window, cx) {
            self.report(err, cx);
        }
    }

    fn report(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error.clone());
        cx.emit(AssetCatalogEvent::Error(error));
        cx.notify();
    }

    // ---------------------------------------------------------------------
    // Edits

    /// Applies an edit in the background (with the project's edit lock),
    /// then reloads; Undo and Redo push the opposite edit.
    fn apply(&mut self, mut edit: Edit, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            self.report("Another change to the catalog is still being made.".into(), cx);
            return;
        }
        self.busy = true;
        self.error = None;
        let before = self.selected.clone();
        let (stash, lock) = (self.stash.clone(), self.lock.clone());
        let applied = edit.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let _guard = lock.lock();
                    edit::apply(&applied, &stash)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(mut inverse) => {
                        // Undoing goes back to what was shown before.
                        inverse.select = Some(before);
                        match step {
                            Step::Do => {
                                this.undo.push(inverse);
                                this.redo.clear();
                            }
                            Step::Undo => this.redo.push(inverse),
                            Step::Redo => this.undo.push(inverse),
                        }
                        if let Some(select) = edit.select.take()
                            && select != this.selected
                        {
                            this.selected = select;
                            this.slot = None;
                        }
                        cx.emit(AssetCatalogEvent::FilesChanged);
                    }
                    Err(err) => {
                        if step == Step::Do {
                            this.report(format!("Couldn’t {}: {err}", edit.name.to_lowercase()), cx);
                        } else {
                            // The files changed under the step: it's gone.
                            this.report(format!("Couldn’t undo or redo {}: {err}", edit.name.to_lowercase()), cx);
                        }
                    }
                }
                this.reload(window, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        if self.busy {
            return Err("an edit is being made".into());
        }
        let edit = self.undo.pop().ok_or("nothing to undo")?;
        self.apply(edit, Step::Undo, window, cx);
        Ok(())
    }

    pub fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        if self.busy {
            return Err("an edit is being made".into());
        }
        let edit = self.redo.pop().ok_or("nothing to redo")?;
        self.apply(edit, Step::Redo, window, cx);
        Ok(())
    }

    /// Where new assets go: the selected folder, or the selected set's.
    fn target_folder(&self) -> PathBuf {
        match self.selected_node() {
            Some(node) if node.is_folder() => node.path.clone(),
            Some(node) => node.path.parent().map(Path::to_path_buf).unwrap_or_else(|| self.root.clone()),
            None => self.root.clone(),
        }
    }

    /// The + menu's New Image Set, New Color Set and New Folder, named as
    /// Xcode names them.
    pub fn add_asset(&mut self, kind: Option<SetKind>, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let dir = self.target_folder();
        let (base, ext, contents, title) = match kind {
            Some(SetKind::Image) => (
                "Image",
                "imageset",
                json!({
                    "images": (["1x", "2x", "3x"].iter().map(|s| json!({ "idiom": "universal", "scale": s })).collect::<Vec<_>>()),
                    "info": xcassets::json::info(),
                }),
                "New Image Set",
            ),
            Some(SetKind::Color) => {
                ("Color", "colorset", json!({ "colors": [{ "idiom": "universal" }], "info": xcassets::json::info() }), "New Color Set")
            }
            None => ("New Folder", "", json!({ "info": xcassets::json::info() }), "New Folder"),
            Some(other) => return Err(format!("zcode can’t create a new {}", other.title())),
        };
        let name = xcassets::unique_name(&dir, base, ext);
        let path = dir.join(if ext.is_empty() { name } else { format!("{name}.{ext}") });
        let edit = Edit {
            name: title.into(),
            ops: vec![Op::CreateDir { path: path.clone() }, Op::write_contents(&path, &contents)],
            select: Some(path),
        };
        self.apply(edit, Step::Do, window, cx);
        Ok(())
    }

    /// Remove Selected Items: the set or folder (and what's in it).
    pub fn remove_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let path = self.selected.clone();
        if path == self.root {
            return Err("select an asset to remove".into());
        }
        let node = self.node(&path).ok_or("no asset is selected")?;
        // Then the next row, or the one before.
        let ix = self.rows.iter().position(|r| r.path == path);
        let next = ix
            .and_then(|ix| {
                self.rows[ix + 1..]
                    .iter()
                    .find(|r| !r.path.starts_with(&path))
                    .or_else(|| self.rows[..ix].last())
                    .map(|r| r.path.clone())
            })
            .unwrap_or_else(|| self.root.clone());
        let name = format!("Remove “{}”", node.name);
        self.apply(Edit { name, ops: vec![Op::Remove { path }], select: Some(next) }, Step::Do, window, cx);
        Ok(())
    }

    /// Renames a set or folder (its folder on disk; the files inside keep
    /// their names, as in Xcode).
    pub fn rename(&mut self, path: &Path, name: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() || name.contains('/') || name.starts_with('.') {
            return Err(format!("“{name}” can’t be an asset’s name."));
        }
        let parent = path.parent().ok_or("can’t rename the catalog")?;
        let ext = match self.node(path).map(|n| &n.kind) {
            Some(NodeKind::Set(set)) => match set.kind {
                SetKind::Other => path.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default(),
                kind => kind.extension().to_string(),
            },
            Some(NodeKind::Folder { .. }) => match path.extension().and_then(|e| e.to_str()) {
                Some(ext) if xcassets::display_name(path) != path.file_name().unwrap_or_default().to_string_lossy() => ext.to_string(),
                _ => String::new(),
            },
            _ => return Err("no such asset".into()),
        };
        let file = if ext.is_empty() { name.to_string() } else { format!("{name}.{ext}") };
        let target = parent.join(&file);
        let taken = std::fs::read_dir(parent)
            .ok()
            .is_some_and(|entries| entries.flatten().any(|e| e.path() != path && xcassets::display_name(&e.path()).eq_ignore_ascii_case(name)));
        if taken {
            return Err(format!("An asset named “{name}” already exists."));
        }
        let edit = Edit {
            name: format!("Rename to “{name}”"),
            ops: vec![Op::Move { from: path.to_path_buf(), to: target.clone() }],
            select: Some(target),
        };
        self.apply(edit, Step::Do, window, cx);
        Ok(())
    }

    /// A file dropped into a well: copied into the set and named by the
    /// slot; the file the slot had goes unless another slot keeps it.
    pub fn drop_on_slot(&mut self, index: usize, file: &Path, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let set_path = self.selected.clone();
        let set = self.selected_set().ok_or("no set is selected")?.clone();
        match set.kind {
            SetKind::Image | SetKind::AppIcon if edit::is_image(file) => {}
            SetKind::Symbol if file.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")) => {}
            SetKind::Image | SetKind::AppIcon | SetKind::Symbol => return Err(format!("“{}” isn’t an image.", file.display())),
            other => return Err(format!("{} wells don’t take files.", other.title())),
        }
        if !file.is_file() {
            return Err(format!("{} isn’t a file", file.display()));
        }
        let mut contents = set.contents.clone();
        let key = set.kind.array_key();
        let entry = contents.get_mut(key).and_then(Value::as_array_mut).and_then(|a| a.get_mut(index)).ok_or("no such slot")?;
        let old = entry.get("filename").and_then(Value::as_str).map(str::to_string);
        let mut ops = vec![];
        let kept = old.as_ref().is_some_and(|old| {
            set.entries().iter().enumerate().any(|(i, e)| i != index && e.get("filename").and_then(Value::as_str) == Some(old))
        });
        let removed = old.clone().filter(|old| !kept && set_path.join(old).exists());
        if let Some(old) = &removed {
            ops.push(Op::Remove { path: set_path.join(old) });
        }
        let dropped = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let name = if removed.as_deref() == Some(dropped.as_str()) { dropped } else { edit::destination_name(&set_path, file) };
        ops.push(Op::Copy { from: file.to_path_buf(), to: set_path.join(&name) });
        entry["filename"] = json!(name);
        ops.push(Op::write_contents(&set_path, &contents));
        self.apply(Edit { name: "Drop Image".into(), ops, select: Some(set_path) }, Step::Do, window, cx);
        self.slot = Some(index);
        Ok(())
    }

    /// Delete on a well: the slot keeps its place, without its file.
    pub fn clear_slot(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let set_path = self.selected.clone();
        let set = self.selected_set().ok_or("no set is selected")?.clone();
        let mut contents = set.contents.clone();
        let entry = contents.get_mut(set.kind.array_key()).and_then(Value::as_array_mut).and_then(|a| a.get_mut(index)).ok_or("no such slot")?;
        let old = entry.as_object_mut().and_then(|e| e.remove("filename")).and_then(|v| v.as_str().map(str::to_string)).ok_or("the slot is empty")?;
        let mut ops = vec![];
        let kept = set.entries().iter().enumerate().any(|(i, e)| i != index && e.get("filename").and_then(Value::as_str) == Some(old.as_str()));
        if !kept && set_path.join(&old).exists() {
            ops.push(Op::Remove { path: set_path.join(&old) });
        }
        ops.push(Op::write_contents(&set_path, &contents));
        self.apply(Edit { name: "Remove Image".into(), ops, select: Some(set_path) }, Step::Do, window, cx);
        Ok(())
    }

    /// Files dropped on the outline or a folder: an image set each (by
    /// name, `name@2x.png` in the 2x well of `name`), as Xcode imports.
    pub fn drop_files(&mut self, folder: Option<PathBuf>, files: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let dir = folder.unwrap_or_else(|| self.target_folder());
        let images: Vec<&PathBuf> = files.iter().filter(|f| f.is_file() && edit::is_image(f)).collect();
        if images.is_empty() {
            return Err("Only images can be dropped into an asset catalog.".into());
        }
        let mut groups: Vec<(String, Vec<(&PathBuf, Option<String>)>)> = vec![];
        for file in images {
            let (base, scale) = edit::scale_of(file);
            match groups.iter_mut().find(|(b, _)| *b == base) {
                Some((_, members)) => members.push((file, scale)),
                None => groups.push((base, vec![(file, scale)])),
            }
        }
        let mut ops = vec![];
        let mut taken: Vec<String> = vec![];
        let mut first = None;
        for (base, members) in groups {
            let mut name = xcassets::unique_name(&dir, &base, "imageset");
            let mut n = 1;
            while taken.iter().any(|t| t.eq_ignore_ascii_case(&name)) {
                name = format!("{base} {n}");
                n += 1;
            }
            taken.push(name.clone());
            let set = dir.join(format!("{name}.imageset"));
            first.get_or_insert(set.clone());
            ops.push(Op::CreateDir { path: set.clone() });
            let vector = members.len() == 1 && edit::is_vector(members[0].0);
            let mut entries = vec![];
            if vector {
                let file = members[0].0;
                let file_name = file.file_name().unwrap_or_default().to_string_lossy().to_string();
                ops.push(Op::Copy { from: file.clone(), to: set.join(&file_name) });
                entries.push(json!({ "filename": file_name, "idiom": "universal" }));
            } else {
                for scale in ["1x", "2x", "3x"] {
                    let mut entry = json!({ "idiom": "universal", "scale": scale });
                    if let Some((file, _)) = members.iter().find(|(_, s)| s.as_deref().unwrap_or("1x") == scale) {
                        let file_name = file.file_name().unwrap_or_default().to_string_lossy().to_string();
                        ops.push(Op::Copy { from: (*file).clone(), to: set.join(&file_name) });
                        entry["filename"] = json!(file_name);
                    }
                    entries.push(entry);
                }
            }
            ops.push(Op::write_contents(&set, &json!({ "images": entries, "info": xcassets::json::info() })));
        }
        self.apply(Edit { name: "Import Images".into(), ops, select: first }, Step::Do, window, cx);
        Ok(())
    }

    fn set_color_component_at(&mut self, set_path: &Path, index: usize, key: &str, value: f64, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        self.edit_color(set_path, index, "Change Color", window, cx, |entry| color::set_component(entry, key, value))
    }

    fn edit_color(
        &mut self,
        set_path: &Path,
        index: usize,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&Value) -> Value,
    ) -> Result<(), String> {
        let set = self.node(set_path).and_then(Node::set).filter(|s| s.kind == SetKind::Color).ok_or("no color set is selected")?.clone();
        let mut contents = set.contents.clone();
        let entry = contents.get_mut("colors").and_then(Value::as_array_mut).and_then(|a| a.get_mut(index)).ok_or("no such color")?;
        *entry = change(entry);
        let edit = Edit { name: name.into(), ops: vec![Op::write_contents(set_path, &contents)], select: Some(set_path.to_path_buf()) };
        self.apply(edit, Step::Do, window, cx);
        Ok(())
    }

    /// The selected color slot, for the inspector's commands.
    fn color_slot(&self, query: Option<&str>) -> Result<usize, String> {
        let slots = self.selected_slots();
        match query {
            Some(q) => slots::find(&slots, q).map(|s| s.index).ok_or_else(|| format!("no slot {q}")),
            None => self.slot.ok_or_else(|| "no color is selected".into()),
        }
    }

    pub fn set_color_component(&mut self, slot: Option<&str>, key: &str, text: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let ix = self.color_slot(slot)?;
        let set = self.selected.clone();
        let entry = self.selected_set().and_then(|s| s.entries().get(ix)).cloned().ok_or("no such color")?;
        let method = if key == "alpha" { InputMethod::Float } else { ColorValue::of(&entry).input_method() };
        let value = method.parse(text).ok_or_else(|| format!("“{text}” isn’t a {} value", method.title()))?;
        self.set_color_component_at(&set, ix, key, value, window, cx)
    }

    pub fn set_color_space(&mut self, slot: Option<&str>, space: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        if !color::COLOR_SPACES.iter().any(|(id, _)| *id == space) {
            return Err(format!("no color space {space}"));
        }
        let (ix, set) = (self.color_slot(slot)?, self.selected.clone());
        self.edit_color(&set, ix, "Change Color Space", window, cx, |e| color::set_color_space(e, space))
    }

    pub fn set_input_method(&mut self, slot: Option<&str>, method: InputMethod, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let (ix, set) = (self.color_slot(slot)?, self.selected.clone());
        self.edit_color(&set, ix, "Change Input Method", window, cx, |e| color::set_input_method(e, method))
    }

    /// Import… from the + menu: an open panel, then as if dropped.
    fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Import".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else { return };
            this.update_in(cx, |this, window, cx| {
                if let Err(err) = this.drop_files(None, &paths, window, cx) {
                    this.report(err, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    // ---------------------------------------------------------------------
    // The control channel

    /// `assets <command>`: `select <asset>`, `slot <slot>`, `expand <folder>`,
    /// `new image|color|folder`, `remove`, `rename <name>`, `drop <slot>
    /// <file>` (a file dropped into a well), `import <file>[;<file>…]`
    /// (dropped on the outline), `clear <slot>`, `color [<slot>] <component>
    /// <value>`, `space <slot> <color space>`, `method <slot> float|8-bit|hex`,
    /// `field name|<component>` (focuses it), `undo`,
    /// `redo`, `reload`.
    pub fn command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let (verb, arg) = command.split_once(' ').unwrap_or((command, ""));
        let arg = arg.trim();
        match verb {
            "select" => self.select_named(arg, window, cx),
            "slot" => {
                let slots = self.selected_slots();
                let slot = slots::find(&slots, arg).ok_or_else(|| format!("no slot {arg}"))?.index;
                self.select_slot(Some(slot), window, cx);
                Ok(())
            }
            "expand" => {
                self.select_named(arg, window, cx)?;
                let path = self.selected.clone();
                self.expanded.insert(path);
                self.rebuild_rows();
                cx.notify();
                Ok(())
            }
            "new" => match arg {
                "image" => self.add_asset(Some(SetKind::Image), window, cx),
                "color" => self.add_asset(Some(SetKind::Color), window, cx),
                "folder" => self.add_asset(None, window, cx),
                other => Err(format!("usage: assets new image|color|folder (not “{other}”)")),
            },
            "remove" => self.remove_selected(window, cx),
            "rename" => {
                let path = self.selected.clone();
                self.rename(&path, arg, window, cx)
            }
            "drop" => {
                let (slot, file) = arg.split_once(' ').ok_or("usage: assets drop <slot> <file>")?;
                let slots = self.selected_slots();
                let ix = slots::find(&slots, slot).ok_or_else(|| format!("no slot {slot}"))?.index;
                self.drop_on_slot(ix, Path::new(file.trim()), window, cx)
            }
            "import" => {
                let files: Vec<PathBuf> = arg.split(';').map(|f| PathBuf::from(f.trim())).collect();
                self.drop_files(None, &files, window, cx)
            }
            "clear" => {
                let slots = self.selected_slots();
                let ix = slots::find(&slots, arg).ok_or_else(|| format!("no slot {arg}"))?.index;
                self.clear_slot(ix, window, cx)
            }
            "color" => {
                let parts: Vec<&str> = arg.split_whitespace().collect();
                match parts.as_slice() {
                    [key, value] => self.set_color_component(None, key, value, window, cx),
                    [slot @ .., key, value] => self.set_color_component(Some(&slot.join(" ")), key, value, window, cx),
                    _ => Err("usage: assets color [<slot>] <component> <value>".into()),
                }
            }
            "space" | "method" => {
                let (slot, value) = arg.rsplit_once(' ').ok_or("usage: assets space|method <slot> <value>")?;
                if verb == "space" {
                    self.set_color_space(Some(slot), value, window, cx)
                } else {
                    let method = InputMethod::from_id(value).ok_or_else(|| format!("no input method {value}"))?;
                    self.set_input_method(Some(slot), method, window, cx)
                }
            }
            "field" => {
                let input = match arg {
                    "name" => self.name.clone(),
                    key => self.components.iter().find(|(k, _, _)| k == key).map(|(_, i, _)| i.clone()).ok_or_else(|| format!("no field {key}"))?,
                };
                input.update(cx, |input, cx| input.focus(window, cx));
                Ok(())
            }
            "undo" => self.undo(window, cx),
            "redo" => self.redo(window, cx),
            "reload" => {
                self.reload(window, cx);
                Ok(())
            }
            other => Err(format!("unknown assets command “{other}”")),
        }
    }

    /// For `dump`'s `asset_catalog`.
    pub fn summary(&self, cx: &App) -> Value {
        let set = self.selected_set();
        let slots: Vec<Value> = set
            .map(|set| {
                set.slots()
                    .iter()
                    .map(|slot| {
                        let file = slot.filename.as_deref().and_then(|f| set.file(f));
                        let path = slot.filename.as_ref().map(|f| self.selected.join(f));
                        let thumbnail = match (&path, file) {
                            (Some(path), Some(_)) => thumbnail_state(cx, path),
                            (Some(_), None) => json!("missing"),
                            _ => Value::Null,
                        };
                        json!({
                            "index": slot.index,
                            "row": slot.row,
                            "label": slot.label,
                            "filename": slot.filename,
                            "color": slot.color.as_ref().map(ColorValue::hex),
                            "thumbnail": thumbnail,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let (cached, bytes, decoded) = ThumbnailCache::usage(cx);
        json!({
            "root": self.root,
            "loaded": self.catalog.is_some(),
            "load_error": self.load_error,
            "sets": self.catalog.as_ref().map(|c| c.set_count),
            "outline": self.rows.iter().map(|r| json!({
                "name": r.name,
                "depth": r.depth,
                "kind": r.kind.map(SetKind::id).unwrap_or("folder"),
            })).collect::<Vec<_>>(),
            "selected": self.selected,
            "selected_name": self.selected_node().map(|n| n.name.clone()),
            "type": self.selected_node().map(Node::type_title),
            "slot": self.slot,
            "slots": slots,
            "unassigned": set.map(|s| s.unassigned().iter().map(|f| f.name.clone()).collect::<Vec<_>>()),
            "properties": set.map(|s| s.properties()),
            "name_field": self.name.read(cx).value().to_string(),
            "components": self.components.iter().map(|(k, i, _)| json!([k, i.read(cx).value().to_string()])).collect::<Vec<_>>(),
            "undo": self.undo.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
            "redo": self.redo.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
            "busy": self.busy,
            "error": self.error,
            "generation": self.generation,
            // Centers, in window points.
            "places": self.places.borrow().iter().map(|(k, b)| (k.clone(), json!([f32::from(b.center().x), f32::from(b.center().y)]))).collect::<serde_json::Map<_, _>>(),
            "thumbnails": { "cached": cached, "bytes": bytes, "decoded": decoded },
        })
    }
}

/// A file's thumbnail state without asking for it.
fn thumbnail_state(cx: &App, path: &Path) -> Value {
    match cx.try_global::<ThumbnailCache>().and_then(|cache| cache.peek(path)) {
        Some(thumbnails::State::Failed(err)) => json!({ "failed": err }),
        Some(state) => json!(state.id()),
        None => json!("none"),
    }
}

fn set_icon(kind: Option<SetKind>) -> IconName {
    match kind {
        None => IconName::Folder,
        Some(SetKind::Image) => IconName::Image,
        Some(SetKind::AppIcon) => IconName::AppWindow,
        Some(SetKind::Color) => IconName::Palette,
        Some(SetKind::Data) => IconName::Database,
        Some(SetKind::Symbol) => IconName::Shapes,
        Some(SetKind::Other) => IconName::FileImage,
    }
}

fn rgba_color(rgba: [f32; 4]) -> Rgba {
    Rgba { r: rgba[0], g: rgba[1], b: rgba[2], a: rgba[3] }
}

/// A thumbnail (or what stands in for one) filling its box.
fn picture<V: 'static>(path: Option<(PathBuf, thumbnails::Stamp)>, cx: &mut Context<V>) -> AnyElement {
    let theme = cx.theme().clone();
    let Some((path, stamp)) = path else { return div().into_any_element() };
    match ThumbnailCache::get(&path, stamp, cx) {
        thumbnails::State::Ready(thumbnail) => {
            img(ImageSource::Render(thumbnail.image.clone())).size_full().object_fit(ObjectFit::Contain).into_any_element()
        }
        thumbnails::State::Loading => div().size_full().into_any_element(),
        thumbnails::State::Failed(_) => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child("No Preview")
            .into_any_element(),
    }
}

impl AssetCatalogView {
    /// A set's first image, for its tile.
    fn tile_picture(node: &Node) -> Option<(PathBuf, thumbnails::Stamp)> {
        let set = node.set().filter(|s| matches!(s.kind, SetKind::Image | SetKind::AppIcon | SetKind::Symbol))?;
        set.entries().iter().filter_map(|e| e.get("filename").and_then(Value::as_str)).find_map(|name| {
            let file = set.file(name)?;
            Some((node.path.join(name), (file.modified, file.len)))
        })
    }

    fn render_outline_rows(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let mut out = vec![];
        for ix in range {
            let Some(row) = self.rows.get(ix).cloned() else { break };
            let selected = row.path == self.selected;
            let (path, toggle) = (row.path.clone(), row.path.clone());
            let drop_dir = if row.folder { row.path.clone() } else { row.path.parent().map(Path::to_path_buf).unwrap_or_else(|| self.root.clone()) };
            let icon = match row.swatch {
                Some(rgba) => div().size(px(12.)).rounded(px(3.)).border_1().border_color(theme.border).bg(rgba_color(rgba)).into_any_element(),
                None => Icon::new(if row.folder && row.expanded { IconName::FolderOpen } else { set_icon(row.kind) })
                    .small()
                    .text_color(if row.folder { theme.primary } else { theme.muted_foreground })
                    .into_any_element(),
            };
            out.push(
                h_flex()
                    .id(("asset-row", ix))
                    .relative()
                    .child(place(&self.places, format!("row:{}", row.name)))
                    .w_full()
                    .h(px(24.))
                    .pl(px(8. + 14. * row.depth as f32))
                    .pr_2()
                    .gap_1p5()
                    .text_sm()
                    .rounded(theme.radius)
                    .when(selected, |el| el.bg(theme.list_active))
                    .hover(|el| el.bg(theme.list_hover))
                    .child(
                        div().w(px(12.)).flex_none().when(row.folder, |el| {
                            el.child(
                                Icon::new(if row.expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                                    .xsmall()
                                    .text_color(theme.muted_foreground),
                            )
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_folder(&toggle, cx);
                            }))
                        }),
                    )
                    .child(icon)
                    .child(div().flex_1().truncate().child(row.name.clone()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.focus_handle.focus(window, cx);
                        this.select_path(path.clone(), window, cx);
                    }))
                    .drag_over::<ExternalPaths>({
                        let accent = theme.primary;
                        move |style, _, _, _| style.bg(accent.opacity(0.15))
                    })
                    .on_drop(cx.listener(move |this, paths: &ExternalPaths, window, cx| {
                        if let Err(err) = this.drop_files(Some(drop_dir.clone()), paths.paths(), window, cx) {
                            this.report(err, cx);
                        }
                    }))
                    .into_any_element(),
            );
        }
        out
    }

    fn render_outline(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let view = cx.entity().downgrade();
        let can_remove = self.selected != self.root && self.selected_node().is_some();
        let root = self.root.clone();
        v_flex()
            .w(px(230.))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                h_flex()
                    .id("asset-catalog-root")
                    .px_3()
                    .py_1p5()
                    .gap_1p5()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(self.selected == self.root, |el| el.bg(theme.list_active))
                    .hover(|el| el.bg(theme.list_hover))
                    .child(Icon::new(IconName::Images).small().text_color(theme.primary))
                    .child(div().truncate().child(xcassets::display_name(&self.root)))
                    .on_click(cx.listener(move |this, _, window, cx| this.select_path(root.clone(), window, cx))),
            )
            .child(
                uniform_list("asset-outline", self.rows.len(), cx.processor(Self::render_outline_rows))
                    .track_scroll(&self.outline_scroll)
                    .flex_1()
                    .px_1(),
            )
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(Button::new("asset-add").icon(IconName::Plus).ghost().xsmall().tooltip("Add New Asset").dropdown_menu(move |menu, _, _| {
                        let (v1, v2, v3, v4) = (view.clone(), view.clone(), view.clone(), view.clone());
                        menu.item(PopupMenuItem::new("Import…").on_click(move |_, window, cx| {
                            v4.update(cx, |this, cx| this.import(window, cx)).ok();
                        }))
                        .separator()
                        .item(PopupMenuItem::new("Image Set").on_click(move |_, window, cx| {
                            v1.update(cx, |this, cx| this.add_asset(Some(SetKind::Image), window, cx).map_err(|e| this.report(e, cx))).ok();
                        }))
                        .item(PopupMenuItem::new("Color Set").on_click(move |_, window, cx| {
                            v2.update(cx, |this, cx| this.add_asset(Some(SetKind::Color), window, cx).map_err(|e| this.report(e, cx))).ok();
                        }))
                        .separator()
                        .item(PopupMenuItem::new("Folder").on_click(move |_, window, cx| {
                            v3.update(cx, |this, cx| this.add_asset(None, window, cx).map_err(|e| this.report(e, cx))).ok();
                        }))
                    }))
                    .child(
                        Button::new("asset-remove")
                            .icon(IconName::Minus)
                            .ghost()
                            .xsmall()
                            .tooltip("Remove Selected Items")
                            .disabled(!can_remove || self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Err(err) = this.remove_selected(window, cx) {
                                    this.report(err, cx);
                                }
                            })),
                    )
                    .child(div().flex_1())
                    .when_some(self.catalog.as_ref().map(|c| c.set_count), |el, count| {
                        el.child(div().text_xs().text_color(theme.muted_foreground).child(match count {
                            1 => "1 asset".to_string(),
                            n => format!("{n} assets"),
                        }))
                    }),
            )
    }

    /// The selected set's wells, row by row.
    fn render_set(&mut self, node: &Node, set: &AssetSet, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let slots = set.slots();
        let mut body = v_flex().id("asset-wells").flex_1().min_h_0().overflow_y_scroll().p_4().gap_4();
        if let Some(err) = &set.error {
            body = body.child(div().text_sm().text_color(theme.danger).child(format!("Contents.json can’t be read: {err}")));
        }
        if set.kind == SetKind::Other || slots.is_empty() {
            body = body.child(div().text_sm().text_color(theme.muted_foreground).child(match set.kind {
                SetKind::Other => format!("{} assets are shown by their files.", node.type_title()),
                _ => "No slots.".to_string(),
            }));
        }
        for (row_ix, (row, wells)) in slots::rows(&slots).into_iter().enumerate() {
            let mut line = h_flex().gap_3().items_start().flex_wrap();
            for slot in wells {
                line = line.child(self.render_well(node, set, slot, cx));
            }
            body = body.child(
                v_flex()
                    .id(("asset-row-group", row_ix))
                    .gap_2()
                    .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme.muted_foreground).child(row))
                    .child(line),
            );
        }
        let unassigned = set.unassigned();
        if set.kind != SetKind::Other && !unassigned.is_empty() {
            body = body.child(
                v_flex()
                    .gap_1()
                    .text_xs()
                    .text_color(theme.warning)
                    .child(h_flex().gap_1().child(Icon::new(IconName::TriangleAlert).xsmall()).child("Unassigned files (no slot names them):"))
                    .children(unassigned.iter().map(|f| div().pl_4().child(f.name.clone()))),
            );
        }
        if set.kind == SetKind::Other {
            body = body.children(set.files.iter().map(|f| div().text_sm().child(f.name.clone())));
        }
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_flex()
                    .px_4()
                    .py_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(Icon::new(set_icon(Some(set.kind))).small().text_color(theme.muted_foreground))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(node.name.clone()))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(set.kind.title())),
            )
            .child(body)
            .into_any_element()
    }

    fn render_well(&mut self, node: &Node, set: &AssetSet, slot: &Slot, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let selected = self.slot == Some(slot.index);
        let index = slot.index;
        let well = div()
            .size(px(WELL))
            .flex_none()
            .rounded(px(6.))
            .border_1()
            .overflow_hidden()
            .when(selected, |el| el.border_2().border_color(theme.primary))
            .when(!selected, |el| el.border_color(theme.border));
        let well = match &slot.color {
            Some(color) => well.child(div().size_full().bg(rgba_color(color.rgba()))),
            None => {
                let file = slot.filename.as_ref().and_then(|name| set.file(name).map(|f| (node.path.join(name), (f.modified, f.len))));
                let missing = slot.filename.is_some() && file.is_none();
                well.bg(theme.secondary).p_1().child(picture(file, cx)).when(missing, |el| {
                    el.child(div().text_xs().text_color(theme.danger).child("Missing file"))
                })
            }
        };
        let droppable = matches!(set.kind, SetKind::Image | SetKind::AppIcon | SetKind::Symbol);
        v_flex()
            .id(("asset-well", index))
            .relative()
            .child(place(&self.places, format!("well:{index}")))
            .w(px(WELL + 16.))
            .items_center()
            .gap_1()
            .child(well)
            .child(div().text_xs().text_center().text_color(theme.muted_foreground).child(slot.label.clone()))
            .when_some(slot.filename.clone(), |el, name| el.child(div().w_full().text_xs().text_center().truncate().child(name)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.focus_handle.focus(window, cx);
                this.select_slot(Some(index), window, cx);
            }))
            .when(droppable, |el| {
                el.drag_over::<ExternalPaths>({
                    let accent = theme.primary;
                    move |style, _, _, _| style.bg(accent.opacity(0.15))
                })
                .on_drop(cx.listener(move |this, paths: &ExternalPaths, window, cx| {
                    let Some(file) = paths.paths().first().cloned() else { return };
                    if let Err(err) = this.drop_on_slot(index, &file, window, cx) {
                        this.report(err, cx);
                    }
                }))
            })
            .into_any_element()
    }

    /// A folder's (or the catalog's) sets and folders as tiles.
    fn render_tiles(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let Some(node) = self.selected_node().cloned() else { return vec![] };
        let mut out = vec![];
        for row in range {
            let mut line = h_flex().gap_3().px_4().py_2();
            for child in node.children.iter().skip(row * TILE_COLUMNS).take(TILE_COLUMNS) {
                let path = child.path.clone();
                let swatch = child.set().filter(|s| s.kind == SetKind::Color).and_then(|s| s.entries().first()).map(|e| ColorValue::of(e).rgba());
                let content = match (swatch, child.is_folder()) {
                    (Some(rgba), _) => div().size_full().bg(rgba_color(rgba)).into_any_element(),
                    (None, true) => div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::Folder).large().text_color(theme.primary))
                        .into_any_element(),
                    (None, false) => match Self::tile_picture(child) {
                        Some(file) => div().size_full().p_1().child(picture(Some(file), cx)).into_any_element(),
                        None => div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(set_icon(child.set().map(|s| s.kind))).text_color(theme.muted_foreground))
                            .into_any_element(),
                    },
                };
                line = line.child(
                    v_flex()
                        .id(SharedString::from(format!("asset-tile-{}", child.path.display())))
                        .relative()
                        .child(place(&self.places, format!("tile:{}", child.name)))
                        .w(px(WELL + 16.))
                        .items_center()
                        .gap_1()
                        .child(div().size(px(WELL)).rounded(px(6.)).border_1().border_color(theme.border).bg(theme.secondary).overflow_hidden().child(content))
                        .child(div().w_full().text_xs().text_center().truncate().child(child.name.clone()))
                        .on_click(cx.listener(move |this, _, window, cx| this.select_path(path.clone(), window, cx))),
                );
            }
            out.push(line.into_any_element());
        }
        out
    }

    fn render_folder(&mut self, node: &Node, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let rows = node.children.len().div_ceil(TILE_COLUMNS);
        let folder = node.path.clone();
        let body = if node.children.is_empty() {
            div()
                .flex_1()
                .p_4()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("No assets. Drop images here, or add a set with + below the outline.")
                .into_any_element()
        } else {
            uniform_list("asset-tiles", rows, cx.processor(Self::render_tiles)).flex_1().into_any_element()
        };
        v_flex()
            .id("asset-folder")
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_flex()
                    .px_4()
                    .py_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(node.name.clone()))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(node.type_title())),
            )
            .child(body)
            .drag_over::<ExternalPaths>({
                let accent = theme.primary;
                move |style, _, _, _| style.bg(accent.opacity(0.08))
            })
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, window, cx| {
                if let Err(err) = this.drop_files(Some(folder.clone()), paths.paths(), window, cx) {
                    this.report(err, cx);
                }
            }))
            .into_any_element()
    }

    fn render_inspector(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let field = |label: &str, value: String| {
            v_flex()
                .gap_0p5()
                .child(div().text_xs().text_color(theme.muted_foreground).child(label.to_string()))
                .child(div().text_sm().child(if value.is_empty() { "—".to_string() } else { value }))
        };
        let mut panel = v_flex()
            .id("asset-inspector")
            .w(px(260.))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .p_3()
            .gap_3()
            .border_l_1()
            .border_color(theme.border);
        let Some(node) = self.selected_node().cloned() else {
            return panel.child(div().text_sm().text_color(theme.muted_foreground).child("No Selection"));
        };
        panel = panel.child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme.muted_foreground).child("ATTRIBUTES"));
        if node.path == self.root {
            panel = panel.child(field("Name", node.name.clone()));
        } else {
            panel = panel.child(
                v_flex()
                    .gap_0p5()
                    .child(div().text_xs().text_color(theme.muted_foreground).child("Name"))
                    .child(div().relative().child(Input::new(&self.name).small().disabled(self.busy)).child(place(&self.places, "field:name".into()))),
            );
        }
        panel = panel.child(field("Type", node.type_title().to_string()));
        match &node.kind {
            NodeKind::Catalog | NodeKind::Folder { .. } => {
                if let NodeKind::Folder { namespace } = node.kind {
                    panel = panel.child(field("Provides Namespace", if namespace { "Yes".into() } else { "No".into() }));
                }
                panel = panel.child(field("Assets", node.sets().len().to_string()));
            }
            NodeKind::Set(set) => {
                for (key, value) in set.properties() {
                    let (label, value) = match key.as_str() {
                        "template-rendering-intent" => ("Render As", if value == "template" { "Template Image".into() } else { "Original Image".to_string() }),
                        "preserves-vector-representation" => ("Preserve Vector Data", if value == "true" { "Yes".into() } else { "No".into() }),
                        "compression-type" => ("Compression", value),
                        "provides-namespace" => ("Provides Namespace", value),
                        "localizable" => ("Localizable", value),
                        _ => (key.as_str(), value),
                    };
                    panel = panel.child(field(label, value));
                }
                if let Some(slot) = self.slot.and_then(|ix| set.slots().into_iter().find(|s| s.index == ix)) {
                    panel = panel.child(div().h(px(1.)).bg(theme.border)).child(field("Slot", slot.title()));
                    panel = match &slot.color {
                        Some(color) => self.render_color_fields(&node, set, &slot, color, panel, cx),
                        None => self.render_file_fields(&node, set, &slot, panel, cx),
                    };
                }
            }
        }
        panel.child(div().flex_1()).child(
            h_flex().gap_2().child(Button::new("asset-finder").label("Show in Finder").xsmall().on_click({
                let path = node.path.clone();
                move |_, _, cx| cx.reveal_path(&path)
            })),
        )
    }

    fn render_file_fields(&mut self, node: &Node, set: &AssetSet, slot: &Slot, mut panel: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme().clone();
        let field = |label: &str, value: String| {
            v_flex()
                .gap_0p5()
                .child(div().text_xs().text_color(theme.muted_foreground).child(label.to_string()))
                .child(div().text_sm().child(if value.is_empty() { "—".to_string() } else { value }))
        };
        let Some(name) = slot.filename.clone() else {
            return panel.child(div().text_sm().text_color(theme.muted_foreground).child("Drop an image here from Finder."));
        };
        let file = set.file(&name).cloned();
        let path = node.path.join(&name);
        let size = cx
            .try_global::<ThumbnailCache>()
            .and_then(|c| c.peek(&path))
            .and_then(|s| match s {
                thumbnails::State::Ready(t) => Some(t.source),
                _ => None,
            });
        panel = panel
            .child(field("File", name.clone()))
            .child(field(
                "Size",
                match size {
                    Some((w, h)) if edit::is_vector(&path) => format!("{w} × {h} points"),
                    Some((w, h)) => format!("{w} × {h} pixels"),
                    None => String::new(),
                },
            ))
            .child(field("File Size", file.map(|f| format_bytes(f.len)).unwrap_or_else(|| "Missing".into())));
        let index = slot.index;
        panel.child(
            h_flex()
                .gap_2()
                .child(Button::new("asset-show-file").label("Show File").xsmall().on_click(move |_, _, cx| cx.reveal_path(&path)))
                .child(Button::new("asset-clear-slot").label("Remove").xsmall().disabled(self.busy).on_click(cx.listener(move |this, _, window, cx| {
                    if let Err(err) = this.clear_slot(index, window, cx) {
                        this.report(err, cx);
                    }
                }))),
        )
    }

    fn render_color_fields(
        &mut self,
        _: &Node,
        _: &AssetSet,
        slot: &Slot,
        color: &ColorValue,
        panel: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme().clone();
        let view = cx.entity().downgrade();
        let index = slot.index.to_string();
        let (space, method) = match color {
            ColorValue::Components { space, .. } => (space.clone(), color.input_method()),
            ColorValue::Unset => ("srgb".to_string(), InputMethod::Float),
            ColorValue::Reference(name) => {
                return panel.child(
                    v_flex()
                        .gap_0p5()
                        .child(div().text_xs().text_color(theme.muted_foreground).child("Content"))
                        .child(div().text_sm().child(format!("System color: {name}"))),
                );
            }
        };
        let (v1, v2, i1, i2) = (view.clone(), view, index.clone(), index);
        let current_space = space.clone();
        let mut panel = panel
            .child(
                h_flex()
                    .gap_2()
                    .child(div().size(px(28.)).rounded(px(4.)).border_1().border_color(theme.border).bg(rgba_color(color.rgba())))
                    .child(div().text_sm().child(color.hex())),
            )
            .child(
                v_flex().gap_0p5().child(div().text_xs().text_color(theme.muted_foreground).child("Content")).child(
                    Button::new("asset-color-space").label(color::color_space_title(&space)).small().outline().dropdown_menu(move |mut menu, _, _| {
                        for (id, title) in color::COLOR_SPACES {
                            let (view, slot) = (v1.clone(), i1.clone());
                            menu = menu.item(PopupMenuItem::new(*title).checked(*id == current_space).on_click(move |_, window, cx| {
                                view.update(cx, |this, cx| this.set_color_space(Some(&slot), id, window, cx).map_err(|e| this.report(e, cx))).ok();
                            }));
                        }
                        menu
                    }),
                ),
            )
            .child(
                v_flex().gap_0p5().child(div().text_xs().text_color(theme.muted_foreground).child("Input Method")).child(
                    Button::new("asset-input-method").label(method.title()).small().outline().dropdown_menu(move |mut menu, _, _| {
                        for m in [InputMethod::Float, InputMethod::Integer, InputMethod::Hex] {
                            let (view, slot) = (v2.clone(), i2.clone());
                            menu = menu.item(PopupMenuItem::new(m.title()).checked(m == method).on_click(move |_, window, cx| {
                                view.update(cx, |this, cx| this.set_input_method(Some(&slot), m, window, cx).map_err(|e| this.report(e, cx))).ok();
                            }));
                        }
                        menu
                    }),
                ),
            );
        for (key, input, _) in &self.components {
            panel = panel.child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(56.)).text_xs().text_color(theme.muted_foreground).child(color::component_title(key)))
                    .child(div().flex_1().relative().child(Input::new(input).small().disabled(self.busy)).child(place(&self.places, format!("field:{key}")))),
            );
        }
        panel
    }
}

fn format_bytes(len: u64) -> String {
    match len {
        n if n < 1024 => format!("{n} bytes"),
        n if n < 1024 * 1024 => format!("{:.1} KB", n as f64 / 1024.),
        n => format!("{:.1} MB", n as f64 / (1024. * 1024.)),
    }
}

impl Render for AssetCatalogView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        ThumbnailCache::release(window, cx);
        self.places.borrow_mut().clear();
        // GPUI hit-tests drop targets only after mouse input, and a drag
        // from Finder isn't any: right after typing, a drop would land
        // nowhere. A file drag moving over the window counts as the mouse.
        let file_drag = canvas(|_, _, _| {}, |_, _, window, _| {
            window.on_mouse_event(|event: &MouseMoveEvent, phase, window, cx| {
                // (A drag of zcode's own starts with the mouse.)
                if phase == DispatchPhase::Bubble && cx.has_active_drag() && window.last_input_was_keyboard() {
                    let position = event.position;
                    window.defer(cx, move |window, cx| {
                        let moved = MouseMoveEvent { position, pressed_button: Some(MouseButton::Left), modifiers: Modifiers::default() };
                        window.dispatch_event(PlatformInput::MouseMove(moved), cx);
                    });
                }
            });
        })
        .absolute()
        .size_0();
        let theme = cx.theme().clone();
        let center = match (self.catalog.clone(), self.selected_node().cloned()) {
            (None, _) => div()
                .flex_1()
                .p_4()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(self.load_error.clone().unwrap_or_else(|| "Reading the catalog…".into()))
                .into_any_element(),
            (Some(_), Some(node)) => match node.set().cloned() {
                Some(set) => self.render_set(&node, &set, cx),
                None => self.render_folder(&node, cx),
            },
            (Some(_), None) => div().flex_1().into_any_element(),
        };
        let view = cx.entity().downgrade();
        v_flex()
            .id("asset-catalog")
            .key_context("AssetCatalog")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .child(file_drag)
            .on_action(cx.listener(|this, _: &Undo, window, cx| {
                this.undo(window, cx).ok();
            }))
            .on_action(cx.listener(|this, _: &Redo, window, cx| {
                this.redo(window, cx).ok();
            }))
            .on_action(cx.listener(|this, _: &PickerSelectNext, window, cx| this.step_selection(true, window, cx)))
            .on_action(cx.listener(|this, _: &PickerSelectPrevious, window, cx| this.step_selection(false, window, cx)))
            .on_action(cx.listener(|this, _: &DeletePlistItem, window, cx| {
                // A well's file, else the set.
                let result = match this.slot.filter(|_| this.selected_set().is_some_and(|s| s.kind != SetKind::Color)) {
                    Some(ix) => this.clear_slot(ix, window, cx),
                    None => this.remove_selected(window, cx),
                };
                if let Err(err) = result {
                    this.report(err, cx);
                }
            }))
            .when_some(self.error.clone(), |el, error| {
                el.child(
                    h_flex()
                        .px_3()
                        .py_1()
                        .gap_2()
                        .text_sm()
                        .bg(theme.danger.opacity(0.12))
                        .text_color(theme.danger)
                        .child(div().flex_1().child(error))
                        .child(Button::new("asset-error-close").icon(IconName::X).ghost().xsmall().on_click(cx.listener(|this, _, _, cx| {
                            this.error = None;
                            cx.notify();
                        }))),
                )
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_outline(cx))
                    .child(center)
                    .child(self.render_inspector(cx)),
            )
            .context_menu(move |menu, _, _| {
                let (v1, v2) = (view.clone(), view.clone());
                menu.item(PopupMenuItem::new("Remove Selected Items").on_click(move |_, window, cx| {
                    v1.update(cx, |this, cx| this.remove_selected(window, cx).map_err(|e| this.report(e, cx))).ok();
                }))
                .item(PopupMenuItem::new("Show in Finder").on_click(move |_, _, cx| {
                    if let Some(path) = v2.upgrade().map(|v| v.read(cx).selected.clone()) {
                        cx.reveal_path(&path);
                    }
                }))
            })
    }
}

//! Live previews of a Markdown or SVG file, in a tab of their own: Editor ▸
//! Open Preview (⇧⌘V) in the focused pane, Open Preview to the Side (⌃⇧⌘V)
//! in the next one (an assistant on the right when there's one pane, as ⌥-
//! click opens). Markdown (`.md`, `.markdown`, and `.mdx` read as plain
//! Markdown) is rendered as rich text by gpui-base's `TextView`, the agent
//! panel's; an SVG is drawn by the image tab (`image_view.rs`). ("Previews"
//! elsewhere in zcode are SwiftUI's, `previews.rs`.)
//!
//! A preview shows its file's buffer, not the file: an edit shows ~150 ms
//! after typing pauses, saved or not. It isn't the file's tab, so opening
//! the file shows its editor (`Item::path` is `None` for a preview,
//! `Item::rendered_path` names its file), and it keeps the buffer open: it
//! goes on following the text after the file's last editor closes. Closing
//! that editor still asks about unsaved changes, and Don't Save puts the
//! saved text back. Renaming or trashing the file closes its previews; the
//! session brings them back.
//!
//! In a Markdown preview, images and links with relative paths are the
//! file's folder's (a leading `/`, the project's root); a link to a file
//! opens it in zcode, any other (http, mailto) in its app. Raw HTML isn't
//! rendered. The preview beside an editor follows it: the block at the
//! editor's top line goes to the top of the preview (as far into it as the
//! line is into its source), and a cursor moved to a block the preview
//! doesn't show scrolls it there. The `TextView`'s blocks are markdown-rs's
//! top-level nodes, so their lines come from parsing the text here with the
//! same options.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::text::{TextView, TextViewState};
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::*;
use parking_lot::Mutex;
use serde_json::{Value, json};
use zcode_editor::EditorState;

use super::image_view::ImageView;
use super::pane::{Item, PaneId, RenderedItem};
use super::Workspace;

/// How long typing pauses before a preview shows the edit.
const EDIT_PAUSE: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderedKind {
    Markdown,
    Svg,
}

impl RenderedKind {
    /// What a preview of `path` would be, if it can have one.
    pub fn of(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "md" | "markdown" | "mdx" => Some(Self::Markdown),
            "svg" => Some(Self::Svg),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Svg => "svg",
        }
    }
}

/// A preview's view.
#[derive(Clone)]
pub enum RenderedView {
    Markdown(Entity<MarkdownView>),
    Svg(Entity<ImageView>),
}

impl RenderedView {
    pub fn entity_id(&self) -> EntityId {
        match self {
            Self::Markdown(view) => view.entity_id(),
            Self::Svg(view) => view.entity_id(),
        }
    }

    pub fn kind(&self) -> RenderedKind {
        match self {
            Self::Markdown(_) => RenderedKind::Markdown,
            Self::Svg(_) => RenderedKind::Svg,
        }
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self {
            Self::Markdown(view) => view.read(cx).focus_handle.clone(),
            Self::Svg(view) => view.read(cx).focus_handle(cx),
        }
    }

    pub fn element(&self) -> AnyElement {
        match self {
            Self::Markdown(view) => view.clone().into_any_element(),
            Self::Svg(view) => view.clone().into_any_element(),
        }
    }

    fn set_text(&self, text: Arc<str>, cx: &mut App) {
        match self {
            Self::Markdown(view) => view.update(cx, |view, cx| view.set_text(text, cx)),
            Self::Svg(view) => view.update(cx, |view, cx| view.set_text(text, cx)),
        }
    }

    /// Lets go of what it drew (the tab closed).
    pub fn release(&self, cx: &mut App) {
        if let Self::Svg(view) = self {
            view.update(cx, |view, cx| view.release(cx));
        }
    }

    fn summary(&self, cx: &App) -> Value {
        match self {
            Self::Markdown(view) => view.read(cx).summary(cx),
            Self::Svg(view) => view.read(cx).summary(),
        }
    }
}

// ---------------------------------------------------------------------------
// Markdown

/// A Markdown file's buffer rendered as rich text.
pub struct MarkdownView {
    path: PathBuf,
    /// Where `/`-rooted links and images are.
    root: PathBuf,
    state: Entity<TextViewState>,
    text: Arc<str>,
    /// Each top-level block's lines in the text (0-based, end exclusive).
    blocks: Vec<Range<u32>>,
    /// The URLs of the text's images.
    images: Vec<String>,
    /// Where images resolved to, kept so a `data:` image is decoded once.
    image_sources: Arc<Mutex<HashMap<SharedUri, ResolvedImage>>>,
    /// The editor's top line and cursor line the preview follows, and
    /// whether it has scrolled to them yet.
    follow: Follow,
    /// Tries again to scroll to them, while the view catches up with the
    /// text.
    _follow_retry: Option<Task<()>>,
    font_size: f32,
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    _parse: Option<Task<()>>,
}

#[derive(Default)]
struct Follow {
    top: Option<u32>,
    cursor: Option<u32>,
    /// What's still to scroll to: the top line, else the cursor's block.
    pending: Option<(u32, bool)>,
    /// Tries left before giving up on `pending`.
    retries: u32,
}

/// How often, and how many times, scrolling waits for the view to have
/// the text's blocks (its parse, then its layout).
const FOLLOW_RETRY: Duration = Duration::from_millis(50);
const FOLLOW_RETRIES: u32 = 40;

impl MarkdownView {
    fn new(path: PathBuf, root: PathBuf, workspace: WeakEntity<Workspace>, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| TextViewState::markdown("", cx));
        Self {
            path,
            root,
            state,
            text: "".into(),
            blocks: vec![],
            images: vec![],
            image_sources: Arc::default(),
            follow: Follow::default(),
            _follow_retry: None,
            font_size: 13.,
            workspace,
            focus_handle: cx.focus_handle(),
            _parse: None,
        }
    }

    pub fn set_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        if self.font_size != size {
            self.font_size = size;
            cx.notify();
        }
    }

    fn set_text(&mut self, text: Arc<str>, cx: &mut Context<Self>) {
        if self.text == text {
            return;
        }
        self.text = text.clone();
        self.state.update(cx, |state, cx| state.set_text(&text, cx));
        self._parse = Some(cx.spawn(async move |this, cx| {
            let (blocks, images) = cx.background_spawn(async move { outline(&text) }).await;
            this.update(cx, |this, cx| {
                this.blocks = blocks;
                this.images = images;
                this.follow.retries = FOLLOW_RETRIES;
                this.scroll_to_followed(cx);
            })
            .ok();
        }));
    }

    /// The editor beside it scrolled (`top`, its first line shown) or its
    /// cursor moved to line `cursor`.
    fn follow_editor(&mut self, top: Option<u32>, cursor: u32, cx: &mut Context<Self>) {
        if top.is_some() && top != self.follow.top {
            self.follow.top = top;
            self.follow.cursor = Some(cursor);
            self.follow.pending = top.map(|top| (top, true));
        } else if self.follow.cursor != Some(cursor) {
            self.follow.cursor = Some(cursor);
            self.follow.pending = Some((cursor, false));
        } else {
            return;
        }
        self.follow.retries = FOLLOW_RETRIES;
        self.scroll_to_followed(cx);
    }

    /// Scrolls to what the editor shows, once the blocks of its text are
    /// known and the view has them.
    fn scroll_to_followed(&mut self, cx: &mut Context<Self>) {
        let Some((line, to_top)) = self.follow.pending else { return };
        let list = self.state.read(cx).list_state().clone();
        if self.blocks.is_empty() || list.item_count() != self.blocks.len() || self.state.read(cx).rendered_text().source() != &*self.text {
            if self.follow.retries > 0 {
                self.follow.retries -= 1;
                self._follow_retry = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(FOLLOW_RETRY).await;
                    this.update(cx, |this, cx| this.scroll_to_followed(cx)).ok();
                }));
            }
            return;
        }
        self.follow.pending = None;
        let (ix, fraction) = block_at(&self.blocks, line);
        if to_top {
            let height = list.bounds_for_item(ix).map_or(0., |b| f32::from(b.size.height));
            let offset = if line == 0 { px(0.) } else { px(height * fraction) };
            list.scroll_to(ListOffset { item_ix: if line == 0 { 0 } else { ix }, offset_in_item: offset });
        } else if list.item_is_above_viewport(ix).unwrap_or(true) || list.item_is_below_viewport(ix).unwrap_or(true) {
            list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) });
        }
        cx.notify();
    }

    fn summary(&self, cx: &App) -> Value {
        let list = self.state.read(cx).list_state();
        let top = list.logical_scroll_top();
        let dir = self.path.parent().unwrap_or(Path::new("/"));
        json!({
            "path": self.path,
            "text": (self.text.len() < 8000).then(|| self.text.to_string()),
            "blocks": self.blocks.iter().map(|b| [b.start + 1, b.end]).collect::<Vec<_>>(),
            "items": list.item_count(),
            "scroll_top": { "item": top.item_ix, "offset": f32::from(top.offset_in_item) },
            "images": self.images.iter().map(|url| {
                let path = local_path(dir, &self.root, url);
                json!({ "url": url, "path": path, "exists": path.as_ref().is_some_and(|p| p.is_file()) })
            }).collect::<Vec<_>>(),
        })
    }
}

impl Render for MarkdownView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let dir: Arc<Path> = self.path.parent().unwrap_or(Path::new("/")).into();
        let root: Arc<Path> = self.root.as_path().into();
        let sources = self.image_sources.clone();
        let images = {
            let (dir, root) = (dir.clone(), root.clone());
            move |uri: &SharedUri| sources.lock().entry(uri.clone()).or_insert_with(|| resolve_image(&dir, &root, uri)).source(uri)
        };
        let workspace = self.workspace.clone();
        let link = move |url: &SharedString, event: &ClickEvent, window: &mut Window, cx: &mut App| {
            if let ClickEvent::Mouse(click) = event
                && !matches!(click.up.button, MouseButton::Left | MouseButton::Middle)
            {
                return;
            }
            match local_path(&dir, &root, url) {
                Some(path) if path.is_file() => {
                    workspace.update(cx, |this, cx| this.open_file(path, None, true, window, cx)).ok();
                }
                Some(_) => {}
                None if url.starts_with('#') => {}
                None => cx.open_url(url),
            }
        };
        v_flex()
            .id("markdown-view")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .text_size(px(self.font_size))
            .child(
                div().size_full().px_6().py_3().child(
                    TextView::new(&self.state).scrollable(true).selectable(true).image_source(images).on_link_click(link).size_full(),
                ),
            )
    }
}

/// The top-level blocks' lines and the images' URLs of Markdown `text`,
/// parsed as `TextView` parses it.
fn outline(text: &str) -> (Vec<Range<u32>>, Vec<String>) {
    use markdown::mdast::Node;
    let mut options = markdown::ParseOptions::gfm();
    options.constructs.math_text = true;
    options.constructs.math_flow = true;
    let Ok(Node::Root(root)) = markdown::to_mdast(text, &options) else { return (vec![], vec![]) };
    let blocks = root
        .children
        .iter()
        .map(|node| node.position().map_or(0..0, |p| p.start.line as u32 - 1..p.end.line as u32))
        .collect();
    fn images(node: &Node, definitions: &HashMap<String, String>, out: &mut Vec<String>) {
        match node {
            Node::Image(image) => out.push(image.url.clone()),
            Node::ImageReference(reference) => out.extend(definitions.get(&reference.identifier).cloned()),
            _ => {}
        }
        for child in node.children().into_iter().flatten() {
            images(child, definitions, out);
        }
    }
    let definitions = root
        .children
        .iter()
        .filter_map(|node| match node {
            Node::Definition(d) => Some((d.identifier.clone(), d.url.clone())),
            _ => None,
        })
        .collect();
    let mut urls = vec![];
    images(&Node::Root(root), &definitions, &mut urls);
    urls.dedup();
    (blocks, urls)
}

/// The block showing `line` (or the next one after it, from a blank line
/// between them), and how far into it the line is.
fn block_at(blocks: &[Range<u32>], line: u32) -> (usize, f32) {
    let ix = blocks.iter().position(|b| b.end > line).unwrap_or(blocks.len().saturating_sub(1));
    let block = &blocks[ix];
    let fraction = if line <= block.start { 0. } else { (line - block.start) as f32 / block.len().max(1) as f32 };
    (ix, fraction.min(1.))
}

/// The file a link or image's `url` names, relative to `dir` (`/…` to
/// `root`); `None` for a URL with a scheme (other than `file:`) or only a
/// fragment.
fn local_path(dir: &Path, root: &Path, url: &str) -> Option<PathBuf> {
    let url = url.trim();
    let file = url.strip_prefix("file://");
    let scheme = url.split_once(':').map(|(scheme, _)| scheme);
    if file.is_none() && scheme.is_some_and(|s| s.len() > 1 && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))) {
        return None;
    }
    let path = file.unwrap_or(url).split(['#', '?']).next().unwrap_or("");
    if path.is_empty() {
        return None;
    }
    let path = percent_decode(path);
    let resolved = match path.strip_prefix('/') {
        Some(rel) if file.is_none() && !Path::new(&path).exists() => root.join(rel),
        Some(_) => PathBuf::from(&path),
        None => dir.join(&path),
    };
    Some(crate::xcodeproj::files::normalize(&resolved))
}

/// `%20` and the like as the bytes they stand for.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |at: usize| bytes.get(at).and_then(|&b| (b as char).to_digit(16));
        if bytes[i] == b'%'
            && let (Some(h), Some(l)) = (hex(i + 1), hex(i + 2))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What an image's URL draws: a file next to the Markdown, an image a
/// `data:` URL carries, or what's at a web address.
#[derive(Clone)]
enum ResolvedImage {
    File(Arc<Path>),
    Data(Arc<Image>),
    Web,
}

impl ResolvedImage {
    fn source(&self, uri: &SharedUri) -> ImageSource {
        match self {
            Self::File(path) => ImageSource::Resource(Resource::Path(path.clone())),
            Self::Data(image) => ImageSource::Image(image.clone()),
            Self::Web => ImageSource::Resource(Resource::Uri(uri.clone())),
        }
    }
}

fn resolve_image(dir: &Path, root: &Path, uri: &str) -> ResolvedImage {
    if uri.starts_with("data:")
        && let Ok(data) = data_url::DataUrl::process(uri)
        && let Some(format) = ImageFormat::from_mime_type(&format!("{}/{}", data.mime_type().type_, data.mime_type().subtype))
        && let Ok((bytes, _)) = data.decode_to_vec()
    {
        return ResolvedImage::Data(Arc::new(Image::from_bytes(format, bytes)));
    }
    match local_path(dir, root, uri) {
        Some(path) => ResolvedImage::File(path.into()),
        None => ResolvedImage::Web,
    }
}

// ---------------------------------------------------------------------------
// The workspace's previews

impl Workspace {
    /// Editor ▸ Open Preview (`side`: to the Side) of the focused tab's
    /// file: its preview shown (made if it has none there), in this pane or
    /// the next. To the side, the editor keeps focus.
    pub(super) fn open_rendered(&mut self, side: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.active_item().map(|item| item.path.clone()) else { return };
        if RenderedKind::of(&path).is_none() {
            return;
        }
        let pane = if side { self.pane_for_open_in_next() } else { self.active_pane };
        let Some(ix) = self.rendered_position(pane, &path).or_else(|| self.add_rendered_item(pane, &path, window, cx)) else { return };
        if side {
            if let Some(p) = self.pane_mut(pane) {
                p.active = ix;
            }
            // It starts where the editor is.
            if let Some(editor) = self.active_editor() {
                self.rendered_follow_editor(&editor, cx);
            }
            cx.notify();
        } else {
            self.activate_item(pane, ix, window, cx);
        }
    }

    /// The tab of `path`'s preview in `pane`.
    fn rendered_position(&self, pane: PaneId, path: &Path) -> Option<usize> {
        self.pane(pane)?.items.iter().position(|item| item.rendered_path() == Some(path))
    }

    /// A preview of `path` at the end of `pane`, its buffer opened (with no
    /// editor, for a preview restored alone) if it wasn't.
    pub(super) fn add_rendered_item(&mut self, pane: PaneId, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> Option<usize> {
        let kind = RenderedKind::of(path)?;
        self.pane(pane)?;
        if self.buffer(path).is_none() && !self.open_buffer_with(path, true, window, cx) {
            return None;
        }
        let view = match kind {
            RenderedKind::Markdown => {
                let (path, root, workspace) = (path.to_path_buf(), self.project.root.clone(), cx.weak_entity());
                RenderedView::Markdown(cx.new(|cx| MarkdownView::new(path, root, workspace, cx)))
            }
            RenderedKind::Svg => RenderedView::Svg(cx.new(|cx| ImageView::svg_text(path.to_path_buf(), cx))),
        };
        let id = view.entity_id();
        let focus = view.focus_handle(cx);
        let subscriptions = vec![cx.on_focus_in(&focus, window, move |this, window, cx| this.view_focused(id, window, cx))];
        if let Some(text) = self.buffer(path).map(|b| Arc::<str>::from(b.editor.read(cx).text().to_string())) {
            view.set_text(text, cx);
        }
        let p = self.pane_mut(pane)?;
        p.items.push(Item::Rendered(RenderedItem { path: path.to_path_buf(), view, _subscriptions: subscriptions }));
        Some(p.items.len() - 1)
    }

    /// The previews of `path`.
    pub(super) fn rendered_views_of(&self, path: &Path) -> Vec<&RenderedView> {
        self.panes
            .iter()
            .flat_map(|pane| pane.items.iter())
            .filter_map(|item| match item {
                Item::Rendered(item) if item.path == path => Some(&item.view),
                _ => None,
            })
            .collect()
    }

    /// `path`'s buffer changed (an edit, or read again from disk): its
    /// previews show it once typing pauses.
    pub(super) fn rendered_edited(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.rendered_views_of(path).is_empty() {
            return;
        }
        let path_buf = path.to_path_buf();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(EDIT_PAUSE).await;
            this.update(cx, |this, cx| {
                let Some(text) = this.buffer(&path_buf).map(|b| Arc::<str>::from(b.editor.read(cx).text().to_string())) else { return };
                for view in this.rendered_views_of(&path_buf).into_iter().cloned().collect::<Vec<_>>() {
                    view.set_text(text.clone(), cx);
                }
            })
            .ok();
        });
        if let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) {
            buffer.rendered_refresh = Some(task);
        }
    }

    /// An editor scrolled or its cursor moved: the Markdown previews of its
    /// file that are shown follow it, if it's shown itself.
    pub(super) fn rendered_follow_editor(&mut self, editor: &Entity<EditorState>, cx: &mut Context<Self>) {
        let shown_previews = || self.panes.iter().filter_map(|p| match p.active_item() {
            Some(Item::Rendered(item)) => match &item.view {
                RenderedView::Markdown(view) => Some((item.path.as_path(), view.clone())),
                RenderedView::Svg(_) => None,
            },
            _ => None,
        });
        if shown_previews().next().is_none() {
            return;
        }
        let shown = self.panes.iter().filter_map(|p| p.active_editor_item()).find(|item| item.editor == *editor);
        let Some(path) = shown.map(|item| item.path.clone()) else { return };
        let (top, cursor) = {
            let state = editor.read(cx);
            (state.visible_rows().map(|rows| rows.start), state.cursor_position().line)
        };
        let views: Vec<Entity<MarkdownView>> = shown_previews().filter(|(p, _)| *p == path).map(|(_, view)| view).collect();
        for view in views {
            view.update(cx, |view, cx| view.follow_editor(top, cursor, cx));
        }
    }

    /// After a preview's tab closed: the buffer closes with it when no tab
    /// shows the file any more.
    pub(super) fn rendered_closed(&mut self, path: &Path, cx: &mut Context<Self>) {
        let open = self.views_of(path).len() + self.diff_views_of(path).len() + self.multibuffer_views_of(path, cx).len() + self.rendered_views_of(path).len();
        if open == 0
            && let Some(ix) = self.buffers.iter().position(|b| b.path == path)
        {
            let buffer = self.buffers.remove(ix);
            self.lsp.update(cx, |lsp, cx| lsp.close_document(&buffer.path, cx));
        }
    }

    /// Markdown previews take the editor's font size.
    pub(super) fn rendered_font_size(&self, view: &RenderedView, cx: &mut App) {
        if let RenderedView::Markdown(view) = view {
            let size = self.font_size;
            view.update(cx, |view, cx| view.set_font_size(size, cx));
        }
    }

    /// `dump`'s `rendered`: every preview, the pane it's in (in layout
    /// order) and whether it's shown.
    pub(super) fn rendered_summary(&self, cx: &App) -> Value {
        let order = self.pane_group.panes();
        let list: Vec<Value> = order
            .iter()
            .enumerate()
            .filter_map(|(n, id)| Some((n, self.pane(*id)?)))
            .flat_map(|(n, pane)| {
                pane.items.iter().enumerate().filter_map(move |(ix, item)| match item {
                    Item::Rendered(item) => {
                        let mut summary = item.view.summary(cx);
                        summary["kind"] = json!(item.view.kind().name());
                        summary["pane"] = json!(n);
                        summary["shown"] = json!(pane.active == ix);
                        summary["title"] = json!(item.title());
                        Some(summary)
                    }
                    _ => None,
                })
            })
            .collect();
        json!(list)
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{block_at, local_path, outline};

    #[test]
    fn blocks_are_the_top_level_nodes_lines() {
        let (blocks, images) = outline("# Title\n\nOne\ntwo\n\n- a\n- b\n\n![logo](img/logo.png)\n\n[r]: ref.png\n![x][r]\n");
        // A list's last item takes the blank line after it.
        assert_eq!(blocks, [0..1, 2..4, 5..8, 8..9, 10..11, 11..12]);
        assert_eq!(images, ["img/logo.png", "ref.png"]);
    }

    #[test]
    fn a_line_is_in_its_block_or_the_next() {
        let blocks = [0..1, 2..6, 8..9];
        assert_eq!(block_at(&blocks, 0), (0, 0.));
        assert_eq!(block_at(&blocks, 1), (1, 0.));
        assert_eq!(block_at(&blocks, 4), (1, 0.5));
        assert_eq!(block_at(&blocks, 20), (2, 1.));
    }

    #[test]
    fn links_resolve_against_the_folder() {
        let (dir, root) = (Path::new("/p/docs"), Path::new("/p"));
        assert_eq!(local_path(dir, root, "other.md#usage"), Some(PathBuf::from("/p/docs/other.md")));
        assert_eq!(local_path(dir, root, "../README.md"), Some(PathBuf::from("/p/README.md")));
        assert_eq!(local_path(dir, root, "my%20file.png"), Some(PathBuf::from("/p/docs/my file.png")));
        assert_eq!(local_path(dir, root, "/assets/a.png"), Some(PathBuf::from("/p/assets/a.png")));
        assert_eq!(local_path(dir, root, "https://example.com/a.png"), None);
        assert_eq!(local_path(dir, root, "mailto:a@b.c"), None);
        assert_eq!(local_path(dir, root, "#heading"), None);
        assert_eq!(local_path(dir, root, "file:///tmp/x.md"), Some(PathBuf::from("/tmp/x.md")));
    }
}

# zcode patch to gpui-base 0.7.3

This is the published `gpui-base` 0.7.3 crate (GPUI Kit's widgets, text
fields among them; Apache-2.0, `LICENSE-APACHE`) with the changes below
(the caret's blink, Markdown images, then the tree's selection), wired in
through `[patch.crates-io]` in zcode's `Cargo.toml`. The unchanged crate is
the commit before this file's.

**Why:** a focused text field's caret (`src/input/base/blink_cursor.rs`)
blinked every 500 ms for as long as the field had focus, and every blink
redraws the window. While a window draws, macOS attributes ~230 MB of
compositor memory to the process ("owned unmapped (graphics)" in `vmmap`,
released ~1.5 s after the last frame), so with a field focused (the Find
field, a picker's query) an idle frontmost window never let go of it, and
kept the main thread waking twice a second. zcode's code editor has its own
copy of this cursor (`crates/zcode-editor/src/base/blink_cursor.rs`, from
the same crate) with the same change.

**The change:**

* `BlinkCursor` counts its blinks since the last `start` (focus,
  activation) or `pause` (input), and after `IDLE_BLINKS` (20, 10 s: GTK's
  `gtk-cursor-blink-timeout`) stops, shown, until the next input. The epoch
  stays, so the next `pause` blinks it again.
* `suspend`, called when the window deactivates, stops a blinking cursor
  (`stop`), and does nothing to one that isn't, so the unfocused fields of a
  window aren't redrawn. `InputState`'s activation observer had the comment
  "Blink the cursor when the window is active, pause when it's not" but only
  started the blink; it (and `OtpState`'s) now suspends it too. GPUI also
  blurs a window's focused element when the window deactivates (an
  inactive window's focus path is empty to focus listeners), which stops the
  blink through `on_blur`, except while the field's context menu is open
  (`on_blur` returns early then).

Each piece is marked `// zcode patch`; `blink_cursor.rs` has a test.

**Updating:** when gpui-kit moves to a newer gpui-base, check whether its
`BlinkCursor` stops blinking after a while without input. If it doesn't,
re-apply the diff below; drop this directory and the `[patch.crates-io]`
entry once it does.

```diff
diff -ru a/src/input/base/blink_cursor.rs b/src/input/base/blink_cursor.rs
--- a/src/input/base/blink_cursor.rs	2026-10-19 18:24:24
+++ b/src/input/base/blink_cursor.rs	2026-10-19 18:24:24
@@ -3,6 +3,11 @@
 
 static INTERVAL: Duration = Duration::from_millis(500);
 static PAUSE_DELAY: Duration = Duration::from_millis(300);
+/// zcode patch: blinks (shown or hidden) after the last input or focus
+/// before the cursor stays shown: 10 s, GTK's `gtk-cursor-blink-timeout`.
+/// Every blink redraws the window, so a cursor that blinks forever keeps an
+/// idle window drawing.
+const IDLE_BLINKS: u32 = 20;
 
 // On Windows, Linux, we should use integer to avoid blurry cursor.
 #[cfg(not(target_os = "macos"))]
@@ -20,6 +25,8 @@
     visible: bool,
     paused: bool,
     epoch: usize,
+    // zcode patch: blinks since the last start or pause.
+    blinks: u32,
 
     _task: Task<()>,
 }
@@ -30,12 +37,14 @@
             visible: false,
             paused: false,
             epoch: 0,
+            blinks: 0,
             _task: Task::ready(()),
         }
     }
 
     /// Start the blinking
     pub(crate) fn start(&mut self, cx: &mut Context<Self>) {
+        self.blinks = 0; // zcode patch
         self.blink(self.epoch, cx);
     }
 
@@ -49,6 +58,16 @@
         cx.notify();
     }
 
+    /// zcode patch: the window went to the background, so stop blinking (an
+    /// inactive window shows no cursor) until [`Self::start`] on activation.
+    /// A no-op on a cursor that isn't blinking, so unfocused inputs don't
+    /// redraw.
+    pub(crate) fn suspend(&mut self, cx: &mut Context<Self>) {
+        if self.epoch != 0 {
+            self.stop(cx);
+        }
+    }
+
     fn next_epoch(&mut self) -> usize {
         self.epoch += 1;
         self.epoch
@@ -65,6 +84,17 @@
             return;
         }
 
+        // zcode patch: idle long enough, so stay shown and stop redrawing. The
+        // epoch stays, so the next pause (input) blinks again.
+        if self.blinks >= IDLE_BLINKS {
+            if !self.visible {
+                self.visible = true;
+                cx.notify();
+            }
+            return;
+        }
+        self.blinks += 1;
+
         self.visible = !self.visible;
         cx.notify();
 
@@ -97,6 +127,7 @@
 
         self.paused = true;
         self.visible = true;
+        self.blinks = 0; // zcode patch
         cx.notify();
 
         // Every pause replaces the pending timer, keeping repeated input visible.
@@ -242,5 +273,37 @@
 
         assert!(!cursor.read_with(cx, |cursor, _| cursor.visible()));
         assert_eq!(notifications.get(), 0, "a stopped cursor kept blinking");
+    }
+
+    // zcode patch
+    #[gpui::test]
+    fn an_idle_cursor_stops_blinking_shown_until_input(cx: &mut TestAppContext) {
+        let cursor = cx.new(|_| BlinkCursor::new());
+        cursor.update(cx, |cursor, cx| cursor.start(cx));
+        cx.run_until_parked();
+
+        let notifications = Rc::new(Cell::new(0usize));
+        let count = notifications.clone();
+        let _observer = cx.update(|cx| cx.observe(&cursor, move |_, _| count.set(count.get() + 1)));
+        cx.executor().advance_clock(INTERVAL * IDLE_BLINKS);
+        cx.run_until_parked();
+        let idle = notifications.get();
+        cx.executor().advance_clock(Duration::from_secs(60));
+        cx.run_until_parked();
+        assert_eq!(notifications.get(), idle, "an idle cursor kept redrawing");
+        assert!(cursor.read_with(cx, |cursor, _| cursor.visible()));
+
+        // Input blinks it again; the window going to the background stops it.
+        cursor.update(cx, |cursor, cx| cursor.pause(cx));
+        cx.run_until_parked();
+        cx.executor().advance_clock(PAUSE_DELAY + INTERVAL * 2);
+        cx.run_until_parked();
+        assert!(notifications.get() > idle + 2);
+        cursor.update(cx, |cursor, cx| cursor.suspend(cx));
+        cx.run_until_parked();
+        let suspended = notifications.get();
+        cx.executor().advance_clock(Duration::from_secs(5));
+        cx.run_until_parked();
+        assert_eq!(notifications.get(), suspended, "a suspended cursor kept blinking");
     }
 }
diff -ru a/src/input/base/state.rs b/src/input/base/state.rs
--- a/src/input/base/state.rs	2026-10-19 18:24:24
+++ b/src/input/base/state.rs	2026-10-19 18:24:50
@@ -791,6 +791,11 @@
                             blink_cursor.start(cx);
                         });
                     }
+                } else {
+                    // zcode patch: the pause the comment above promises. GPUI's
+                    // blur on deactivation stops it too, but not while the
+                    // context menu is open (`on_blur` returns early).
+                    input.blink_cursor.update(cx, |blink_cursor, cx| blink_cursor.suspend(cx));
                 }
             }),
             cx.on_focus(&focus_handle, window, Self::on_focus),
diff -ru a/src/otp_input.rs b/src/otp_input.rs
--- a/src/otp_input.rs	2026-10-19 18:24:24
+++ b/src/otp_input.rs	2026-10-19 18:24:24
@@ -35,6 +35,9 @@
             cx.observe_window_activation(window, |this, window, cx| {
                 if window.is_window_active() && this.focus_handle.is_focused(window) {
                     this.blink_cursor.update(cx, |cursor, cx| cursor.start(cx));
+                } else if !window.is_window_active() {
+                    // zcode patch: pause in the background.
+                    this.blink_cursor.update(cx, |cursor, cx| cursor.suspend(cx));
                 }
             }),
             cx.on_focus(&focus_handle, window, Self::on_focus),
```

## Markdown images

**Why:** in zcode's Markdown previews (the `TextView`), images on one line
with only spaces between them (`![a](a.png) ![b](b.png)`) went through the
inline flow, which sizes every image to the text's line (a 400×300 image
drawn at about 23×17 pt), and an image that couldn't be drawn (a missing,
empty or corrupt file) left nothing at all.

**The change:**

* A flow that is only images and the spaces between them draws them at
  their own size (`images_only`), no wider than the flow, keeping their
  shape (`fit_images`); images in a line of text keep the text's height.
* An image that can't be drawn shows an `ImagePlaceholder` (`node.rs`): its
  alt text, else its file name, in a muted, bordered box, through `img`'s
  `with_fallback`, in a paragraph of its own and inside a flow. In a flow
  its measured size is the placeholder's. An image alone in its paragraph
  sits in a row of its own so its placeholder is as wide as its text.

Each piece is marked `// zcode patch`; `inline_flow.rs` has a test.

**Updating:** check whether the new gpui-base sizes a row of images by the
images and draws something for an image that fails to load; re-apply the
diff below where it doesn't.

```diff
diff -ru a/src/text/inline_flow.rs b/src/text/inline_flow.rs
--- a/src/text/inline_flow.rs
+++ b/src/text/inline_flow.rs
@@ -19,7 +19,7 @@ use crate::text::text_view::{LinkClickHandlerFn, handle_link_click};
 use super::{
     inline::{Inline, InlineHighlight, InlineState, text_runs, text_size_ranges},
     inline_object::{InlineObject, MeasuredInlineObject},
-    node::LinkMark,
+    node::{ImagePlaceholder, LinkMark},
     range_highlight::RevealAt,
 };
 
@@ -59,6 +59,8 @@ pub(super) enum InlineFlowItem {
     },
     Image {
         source: ImageSource,
+        /// zcode patch: drawn instead when the image can't be.
+        placeholder: ImagePlaceholder,
         link: Option<LinkMark>,
         title: String,
         width: Option<DefiniteLength>,
@@ -215,6 +217,7 @@ enum MeasureItem {
     },
     Image {
         source: ImageSource,
+        placeholder: ImagePlaceholder,
         width: Option<DefiniteLength>,
         height: Option<DefiniteLength>,
     },
@@ -257,13 +260,16 @@ impl InlineFlow {
     fn image_element(
         ix: usize,
         source: &ImageSource,
+        placeholder: &ImagePlaceholder,
         link: &Option<LinkMark>,
         _title: &str,
         size: Size<Pixels>,
         link_click_handler: Option<Arc<LinkClickHandlerFn>>,
     ) -> AnyElement {
+        let placeholder = placeholder.clone(); // zcode patch
         img(source.clone())
             .id(ix)
+            .with_fallback(move || placeholder.element())
             .object_fit(ObjectFit::Contain)
             .max_w(relative(1.))
             .w(size.width)
@@ -339,19 +345,23 @@ impl Element for InlineFlow {
             rem_size: window.rem_size(),
             line_height: window.line_height(),
         };
+        let natural = images_only(&measure_items); // zcode patch
         let image_sizes = measure_items
             .iter()
             .enumerate()
             .map(|(ix, item)| match item {
                 MeasureItem::Image {
                     source,
+                    placeholder,
                     width,
                     height,
                 } => Some(measure_image_size(
                     ix,
                     source,
+                    placeholder,
                     *width,
                     *height,
+                    natural,
                     typography.line_height,
                     typography.rem_size,
                     window,
@@ -391,13 +401,14 @@ impl Element for InlineFlow {
                 }
                 let state = &mut *state;
                 let key = state.key.as_ref().expect("set by request_layout");
+                let image_sizes = fit_images(&key.image_sizes, wrap_width); // zcode patch
                 let layout = window.with_rem_size(Some(typography.rem_size), |window| {
                     window.with_text_style(
                         Some(typography.text_style.subtract(&Default::default())),
                         |window| {
                             layout_measured_flow(
                                 &key.items,
-                                &key.image_sizes,
+                                &image_sizes,
                                 &objects,
                                 &typography.text_style,
                                 wrap_width,
@@ -615,6 +626,7 @@ impl Element for InlineFlow {
                 } => {
                     let InlineFlowItem::Image {
                         source,
+                        placeholder,
                         link,
                         title,
                         ..
@@ -626,6 +638,7 @@ impl Element for InlineFlow {
                     let mut element = Self::image_element(
                         elements.len(),
                         source,
+                        placeholder,
                         link,
                         title.as_str(),
                         fragment_size,
@@ -712,11 +725,13 @@ impl From<&InlineFlowItem> for MeasureItem {
             },
             InlineFlowItem::Image {
                 source,
+                placeholder,
                 width,
                 height,
                 ..
             } => MeasureItem::Image {
                 source: source.clone(),
+                placeholder: placeholder.clone(),
                 width: *width,
                 height: *height,
             },
@@ -770,19 +785,23 @@ pub(super) fn intrinsic_width(
     let items = items.iter().map(MeasureItem::from).collect::<Vec<_>>();
     let line_height = window.line_height();
     let rem_size = window.rem_size();
+    let natural = images_only(&items); // zcode patch
     let images = items
         .iter()
         .enumerate()
         .map(|(ix, item)| match item {
             MeasureItem::Image {
                 source,
+                placeholder,
                 width,
                 height,
             } => Some(measure_image_size(
                 ix,
                 source,
+                placeholder,
                 *width,
                 *height,
+                natural,
                 line_height,
                 rem_size,
                 window,
@@ -1507,35 +1526,79 @@ fn push_text_wrap_fragments<'a>(
     }
 }
 
+/// zcode patch: whether a flow is only images (and the spaces between them): a row of
+/// pictures, drawn at their own size, rather than images in a line of text,
+/// drawn at the text's height.
+fn images_only(items: &[MeasureItem]) -> bool {
+    items.iter().any(|item| matches!(item, MeasureItem::Image { .. }))
+        && items.iter().all(|item| match item {
+            MeasureItem::Image { .. } => true,
+            MeasureItem::Text { text, .. } => text.trim().is_empty(),
+            MeasureItem::Object { .. } => false,
+        })
+}
+
+/// zcode patch: images no wider than the flow, keeping their shape.
+fn fit_images(sizes: &[Option<Size<Pixels>>], wrap_width: Option<Pixels>) -> Vec<Option<Size<Pixels>>> {
+    sizes
+        .iter()
+        .map(|image| {
+            image.map(|image| match wrap_width {
+                Some(max) if image.width > max && max > Pixels::ZERO => {
+                    size(max, image.height * (max / image.width))
+                }
+                _ => image,
+            })
+        })
+        .collect()
+}
+
 #[allow(clippy::too_many_arguments)]
 fn measure_image_size(
     ix: usize,
     source: &ImageSource,
+    placeholder: &ImagePlaceholder,
     width: Option<DefiniteLength>,
     height: Option<DefiniteLength>,
+    natural: bool,
     line_height: Pixels,
     rem_size: Pixels,
     window: &mut Window,
     cx: &mut App,
 ) -> Size<Pixels> {
-    let intrinsic_size = if width.is_some() && height.is_some() {
-        None
-    } else {
-        intrinsic_image_size(ix, source, width, height, window, cx)
-    };
-    image_size(width, height, intrinsic_size, line_height, rem_size)
+    let intrinsic = intrinsic_image_size(ix, source, placeholder, width, height, window, cx);
+    match intrinsic {
+        // zcode patch: what can't be drawn shows its placeholder at the
+        // placeholder's size; a row of images, the images' own size.
+        Some((placeholder_size, true)) => placeholder_size,
+        Some((intrinsic, false)) if natural && width.is_none() && height.is_none() => intrinsic,
+        _ if width.is_some() && height.is_some() => image_size(width, height, None, line_height, rem_size),
+        intrinsic => image_size(width, height, intrinsic.map(|(size, _)| size), line_height, rem_size),
+    }
 }
 
+/// The image's size as GPUI lays it out, and (zcode patch) whether that's its
+/// placeholder's (it couldn't be loaded); `None` while it loads.
 fn intrinsic_image_size(
     ix: usize,
     source: &ImageSource,
+    placeholder: &ImagePlaceholder,
     width: Option<DefiniteLength>,
     height: Option<DefiniteLength>,
     window: &mut Window,
     cx: &mut App,
-) -> Option<Size<Pixels>> {
+) -> Option<(Size<Pixels>, bool)> {
+    let failed = Rc::new(std::cell::Cell::new(false));
+    let placeholder = placeholder.clone();
     let mut element = img(source.clone())
         .id(ix)
+        .with_fallback({
+            let failed = failed.clone();
+            move || {
+                failed.set(true);
+                placeholder.element()
+            }
+        })
         .object_fit(ObjectFit::Contain)
         .max_w(relative(1.))
         .when_some(width, |this, width| this.w(width))
@@ -1546,7 +1609,7 @@ fn intrinsic_image_size(
     if measured_size.width <= Pixels::ZERO || measured_size.height <= Pixels::ZERO {
         None
     } else {
-        Some(measured_size)
+        Some((measured_size, failed.get()))
     }
 }
 
@@ -1652,6 +1715,26 @@ mod tests {
     use super::*;
     use gpui::SharedUri;
 
+    /// A row of images (spaces between them) is drawn at the images' own
+    /// size, no wider than the flow; an image in text isn't.
+    #[test]
+    fn rows_of_images_keep_their_size_within_the_width() {
+        let image = || MeasureItem::Image {
+            source: SharedUri::from("https://example.com/a.png").into(),
+            placeholder: ImagePlaceholder::for_test(),
+            width: None,
+            height: None,
+        };
+        let text = |text: &str| MeasureItem::Text { text: text.to_string().into(), links: vec![], highlights: vec![] };
+        assert!(images_only(&[image(), text(" "), image()]));
+        assert!(images_only(&[image()]));
+        assert!(!images_only(&[image(), text(" and "), image()]));
+        assert!(!images_only(&[text("   ")]));
+        let sizes = [Some(size(px(400.), px(300.))), None, Some(size(px(100.), px(50.)))];
+        assert_eq!(fit_images(&sizes, Some(px(200.))), [Some(size(px(200.), px(150.))), None, Some(size(px(100.), px(50.)))]);
+        assert_eq!(fit_images(&sizes, None), sizes);
+    }
+
     mod frames {
         use super::super::FLOW_LAYOUTS;
         use crate::text::{TextView, TextViewState};
@@ -1868,6 +1951,7 @@ mod tests {
             },
             MeasureItem::Image {
                 source: SharedUri::from("https://example.com/badge.png").into(),
+                placeholder: ImagePlaceholder::for_test(),
                 width: None,
                 height: None,
             },
@@ -1960,6 +2044,7 @@ mod tests {
             let items = vec![
                 MeasureItem::Image {
                     source: SharedUri::from("https://example.com/icon.png").into(),
+                    placeholder: ImagePlaceholder::for_test(),
                     width: None,
                     height: None,
                 },
diff -ru a/src/text/node.rs b/src/text/node.rs
--- a/src/text/node.rs
+++ b/src/text/node.rs
@@ -707,6 +707,45 @@ impl ImageNode {
     }
 }
 
+/// zcode patch: what an image that can't be drawn (missing, empty or corrupt) shows in
+/// its place: its alt text, else its file name, in a muted box.
+#[derive(Clone, Debug, PartialEq)]
+pub(crate) struct ImagePlaceholder {
+    text: SharedString,
+    color: Hsla,
+    border: Hsla,
+}
+
+impl ImagePlaceholder {
+    pub(crate) fn new(image: &ImageNode, style: &TextViewStyle) -> Self {
+        let alt = image.alt.clone().filter(|alt| !alt.trim().is_empty());
+        let text = alt.unwrap_or_else(|| {
+            let url = image.url.as_ref().split(['?', '#']).next().unwrap_or_default();
+            let name = url.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
+            SharedString::from(if name.is_empty() || url.starts_with("data:") { "image".to_string() } else { name.to_string() })
+        });
+        Self { text, color: style.muted_foreground(), border: style.border() }
+    }
+
+    #[cfg(test)]
+    pub(crate) fn for_test() -> Self {
+        Self { text: "image".into(), color: Hsla::default(), border: Hsla::default() }
+    }
+
+    pub(crate) fn element(&self) -> AnyElement {
+        div()
+            .flex_none()
+            .px_1()
+            .rounded_sm()
+            .border_1()
+            .border_color(self.border)
+            .text_color(self.color)
+            .whitespace_nowrap()
+            .child(self.text.clone())
+            .into_any_element()
+    }
+}
+
 impl std::fmt::Debug for ImageNode {
     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
         f.debug_struct("ImageNode")
@@ -2545,9 +2584,13 @@ impl Paragraph {
                     );
                 }
                 let link_click_handler = node_cx.link_click_handler.clone();
-                child_nodes.push(
+                let placeholder = ImagePlaceholder::new(image, &node_cx.style);
+                // zcode patch: a placeholder for an image that can't be drawn, in a
+                // row of its own, so it's as wide as its text rather than the paragraph.
+                child_nodes.push(div().flex().child(
                     img(node_cx.image_source(image))
                         .id(ix)
+                        .with_fallback(move || placeholder.element())
                         .object_fit(ObjectFit::Contain)
                         .max_w(relative(1.))
                         .when_some(image.width, |this, width| this.w(width))
@@ -2578,9 +2621,8 @@ impl Paragraph {
                                         cx,
                                     );
                                 })
-                        })
-                        .into_any_element(),
-                );
+                        }),
+                ).into_any_element());
 
                 consumed += text.len();
                 text.clear();
@@ -2758,6 +2800,7 @@ impl Paragraph {
 
                 items.push(InlineFlowItem::Image {
                     source: node_cx.image_source(image),
+                    placeholder: ImagePlaceholder::new(image, &node_cx.style), // zcode patch
                     link: image.link.clone(),
                     title: image.title(),
                     width: image.width,

## Tree selection across folders opening and closing

**Why:** `TreeState` keeps its selection as a row index, and opening or
closing a folder rebuilds the rows without moving it, so the project
navigator highlighted whichever file now had that row (closing the folder of
the file shown, or opening a folder above it).

**The change:** `rebuild_entries` keeps the selected item's id and selects
its new row afterwards, or nothing when it's now hidden. Marked
`// zcode patch`, with a test. Drop it once `TreeState` keeps its selection
by item.

```diff
--- a/src/tree.rs	2026-10-20 21:58:23
+++ b/src/tree.rs	2026-10-20 21:58:23
@@ -367,12 +367,17 @@
     }
 
     fn rebuild_entries(&mut self) {
+        // zcode patch: the selection stays on its item wherever a folder
+        // opening or closing moves it, and goes when it's hidden, rather than
+        // staying on a row that's now another item.
+        let selected = self.selected_item().map(|item| item.id.clone());
         let roots = std::mem::take(&mut self.entries)
             .into_iter()
             .filter(TreeEntry::is_root)
             .map(|entry| entry.item)
             .collect::<Vec<_>>();
         self.replace_items(roots);
+        self.selected_ix = selected.and_then(|id| self.index_of(&id));
     }
 
     fn on_action_confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
@@ -688,6 +693,27 @@
             assert_eq!(state.entries.len(), 2);
             state.toggle_expand(0, cx);
             assert_eq!(state.entries.len(), 1);
+        });
+    }
+
+    // zcode patch
+    #[gpui::test]
+    fn the_selection_follows_its_item_and_goes_when_hidden(cx: &mut gpui::TestAppContext) {
+        let docs = TreeItem::new("docs", "docs").child(TreeItem::new("docs/a.md", "a.md"));
+        let src = TreeItem::new("src", "src")
+            .expanded(true)
+            .child(TreeItem::new("src/lib.rs", "lib.rs"));
+        let state = cx.new(|cx| TreeState::new(cx).items(vec![docs, src]));
+
+        state.update(cx, |state, cx| {
+            state.set_selected_index(state.index_of(&"src/lib.rs".into()), cx);
+            // A folder above it opens: the selection moves down with it.
+            state.toggle_expand(0, cx);
+            assert_eq!(state.selected_item().map(|item| item.id.as_str()), Some("src/lib.rs"));
+            // Its own folder closes: nothing is selected.
+            let src = state.index_of(&"src".into()).unwrap();
+            state.toggle_expand(src, cx);
+            assert_eq!(state.selected_index(), None);
         });
     }
 
```

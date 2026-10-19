# zcode patch to gpui-pre-macos 0.3.9

This is the published `gpui-pre-macos` 0.3.9 crate (GPUI's macOS backend,
Zed's `gpui_macos` at `93d74566`; Apache-2.0, `LICENSE-APACHE`) with the
change below, wired in through `[patch.crates-io]` in zcode's `Cargo.toml`.
The unchanged crate is the commit before this file's.

**Why:** a window's frame source (a `CVDisplayLink` subscription, in
`src/display_link.rs`) only stopped while the window was occluded. A visible
window that had nothing to draw still woke the main thread on every display
refresh, 120 times a second here, to find nothing dirty: ~1% of a core and
~150 wakeups a second per idle window (the welcome window: 0.29 s of CPU per
30 s idle).

**The change (`src/window.rs`, `src/display_link.rs`):** GPUI already has
the other half. Its core calls `PlatformWindow::frame_waker` whenever a
window gains frame demand (a view notified, the window refreshed, a
next-frame callback or animation frame asked for, a throttled frame to
retry; see `WindowInvalidator` in gpui-pre's `src/window.rs`), for
platforms that stop asking for frames while idle (upstream: the web). The
macOS window now provides one:

* `step` counts the frame requests since the last demand, and once there
  have been `IDLE_FRAMES_BEFORE_PAUSE` (4: the frame that served the demand
  and three more) over `IDLE_TIME_BEFORE_PAUSE` (33 ms) at least, stops the
  frame source (`note_frame_request`, marking it `paused_while_idle`). The
  time matters: a restarted display link can tick in a burst, and typing a
  key every few milliseconds mustn't pause it between keys.
* The waker (`frame_waker` → `frame_demanded`) resets the count and, if the
  source was paused for idleness, starts it again and asks for one frame on
  the main queue at once (`WindowFrameSource::request_frame`, the same
  `merge_data` the display link's ticks use), so the first frame after a
  pause doesn't wait for the restarted link's first tick. A source stopped
  because the window is occluded stays stopped: the occlusion, screen and
  key-window handlers start it as before.
* Every other start (occlusion, screen change, activation, `displayLayer:`)
  resets the count; every stop clears the paused mark.

The waker takes the window state's lock with `try_lock` and, should it ever
be called while the platform holds that lock, resumes from a task on the
main thread instead. GPUI calls it from app code, never from inside the
platform's callbacks while they hold the lock.

Input still reaches the window while it's paused (AppKit delivers events,
not the display link), and whatever it changes notifies a view, which wakes
the source. What's lost is GPUI's presenting of unchanged frames for a
second after high-rate input (`InputRateTracker`), which keeps a ProMotion
display from lowering its refresh rate during input that redraws nothing;
input that redraws something wakes the source as usual.

The crate's `Cargo.toml` also allows the `deprecated` lint (its `cocoa`
calls): as a path dependency its warnings are no longer capped as a
registry crate's are.

Each piece is marked `// zcode patch`.

**Updating:** when gpui-kit moves to a newer GPUI, check whether the macOS
window implements `frame_waker` (or otherwise pauses its display link while
idle). If it doesn't, re-apply the diff below; drop this directory and the
`[patch.crates-io]` entry once it does.

```diff
diff -ru a/Cargo.toml b/Cargo.toml
--- a/Cargo.toml	2006-07-24 03:21:28
+++ b/Cargo.toml	2026-10-19 00:50:42
@@ -50,6 +50,11 @@
     "gpui_apple/test-support",
 ]
 
+# zcode patch: as a path dependency this crate's lints are no longer capped
+# the way a registry crate's are; upstream's Zed workspace allows these.
+[lints.rust]
+deprecated = "allow"
+
 [lib]
 name = "gpui_macos"
 path = "src/gpui_macos.rs"
diff -ru a/src/display_link.rs b/src/display_link.rs
--- a/src/display_link.rs	2006-07-24 03:21:28
+++ b/src/display_link.rs	2026-10-19 00:48:48
@@ -333,6 +333,22 @@
         Ok(())
     }
 
+    /// Whether the source is subscribed to its display's link.
+    ///
+    /// zcode patch: windows pause their source while idle.
+    pub fn is_running(&self) -> bool {
+        self.registration.is_some()
+    }
+
+    /// Asks for one frame on the main queue now, as a tick of the display's
+    /// link would (ticks coalesce with it).
+    ///
+    /// zcode patch: a paused window resumes with a frame at once instead of
+    /// waiting for the restarted link's first tick.
+    pub fn request_frame(&self) {
+        self.frame_requests.merge_data(1);
+    }
+
     pub fn take_signal(&self) -> Option<(Instant, FrameRequestSource)> {
         self.signal.take()
     }
diff -ru a/src/window.rs b/src/window.rs
--- a/src/window.rs	2006-07-24 03:21:28
+++ b/src/window.rs	2026-10-19 03:45:57
@@ -655,6 +655,14 @@
     }
 }
 
+/// zcode patch: frame requests in a row with no frame demand before a
+/// window's frame source pauses, and how long since the last demand at
+/// least: a restarted display link can tick several times in a burst, and
+/// typing a key every few milliseconds mustn't pause it between keys. The
+/// frame that served the last demand counts as the first request.
+const IDLE_FRAMES_BEFORE_PAUSE: u32 = 4;
+const IDLE_TIME_BEFORE_PAUSE: std::time::Duration = std::time::Duration::from_millis(33);
+
 struct MacWindowState {
     handle: AnyWindowHandle,
     foreground_executor: ForegroundExecutor,
@@ -666,6 +674,12 @@
     cursor_style: CursorStyle,
     cursor_visible: Arc<AtomicBool>,
     frame_source: Option<WindowFrameSource>,
+    // zcode patch: frame requests since the last frame demand (GPUI's frame
+    // waker) and when that was, and whether the frame source was paused for
+    // want of any.
+    idle_frames: u32,
+    last_demand: std::time::Instant,
+    paused_while_idle: bool,
     renderer: renderer::Renderer,
     request_frame_callback: Option<Box<dyn FnMut(RequestFrameOptions)>>,
     event_callback: Option<Box<dyn FnMut(PlatformInput) -> gpui::DispatchEventResult>>,
@@ -835,6 +849,8 @@
 
     fn start_display_link(&mut self) {
         self.stop_display_link();
+        self.idle_frames = 0;
+        self.last_demand = std::time::Instant::now();
         unsafe {
             if !self
                 .native_window
@@ -856,11 +872,50 @@
     }
 
     fn stop_display_link(&mut self) {
+        self.paused_while_idle = false;
         if let Some(frame_source) = self.frame_source.as_mut() {
             frame_source.stop();
         }
     }
 
+    /// zcode patch: counts a frame request, and pauses the frame source once
+    /// `IDLE_FRAMES_BEFORE_PAUSE` have come without any frame demand, over
+    /// `IDLE_TIME_BEFORE_PAUSE` at least, so an idle window stops waking the
+    /// main thread on every display refresh.
+    fn note_frame_request(&mut self) {
+        self.idle_frames = self.idle_frames.saturating_add(1);
+        if self.idle_frames >= IDLE_FRAMES_BEFORE_PAUSE
+            && self.last_demand.elapsed() >= IDLE_TIME_BEFORE_PAUSE
+            && self
+                .frame_source
+                .as_ref()
+                .is_some_and(WindowFrameSource::is_running)
+        {
+            self.stop_display_link();
+            self.paused_while_idle = true;
+        }
+    }
+
+    /// zcode patch: frame demand (a view notified, the window refreshed, a
+    /// next-frame callback or animation frame asked for). Restarts a paused
+    /// frame source and asks for a frame at once; a source stopped for
+    /// occlusion stays stopped until the window is visible again.
+    fn frame_demanded(&mut self) {
+        self.idle_frames = 0;
+        self.last_demand = std::time::Instant::now();
+        if !self.paused_while_idle {
+            return;
+        }
+        self.start_display_link();
+        if let Some(frame_source) = self
+            .frame_source
+            .as_ref()
+            .filter(|frame_source| frame_source.is_running())
+        {
+            frame_source.request_frame();
+        }
+    }
+
     fn is_maximized(&self) -> bool {
         fn rect_to_size(rect: NSRect) -> Size<Pixels> {
             let NSSize { width, height } = rect.size;
@@ -1100,6 +1155,9 @@
                 cursor_style: CursorStyle::Arrow,
                 cursor_visible,
                 frame_source: None,
+                idle_frames: 0,
+                last_demand: std::time::Instant::now(),
+                paused_while_idle: false,
                 renderer: renderer::new_renderer(
                     renderer_context,
                     native_window as *mut _,
@@ -2022,6 +2080,27 @@
         self.0.as_ref().lock().request_frame_callback = Some(callback);
     }
 
+    // zcode patch: GPUI calls this whenever a window gains frame demand, which
+    // resumes a frame source paused while the window was idle.
+    fn frame_waker(&self) -> Option<Rc<dyn Fn()>> {
+        let state = Arc::downgrade(&self.0);
+        let executor = self.0.lock().foreground_executor.clone();
+        Some(Rc::new(move || {
+            let Some(state) = state.upgrade() else {
+                return;
+            };
+            // GPUI wakes from app code, never while the window's state is
+            // locked; should that change, resume once the holder is done.
+            if let Some(mut lock) = state.try_lock() {
+                lock.frame_demanded();
+            } else {
+                executor
+                    .spawn(async move { state.lock().frame_demanded() })
+                    .detach();
+            }
+        }))
+    }
+
     fn on_input(&self, callback: Box<dyn FnMut(PlatformInput) -> gpui::DispatchEventResult>) {
         self.0.as_ref().lock().event_callback = Some(callback);
     }
@@ -3359,7 +3438,9 @@
             signal_source,
             ..Default::default()
         });
-        window_state.lock().request_frame_callback = Some(callback);
+        let mut lock = window_state.lock();
+        lock.request_frame_callback = Some(callback);
+        lock.note_frame_request();
     }
 }
 
```

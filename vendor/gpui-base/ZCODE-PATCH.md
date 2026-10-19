# zcode patch to gpui-base 0.7.3

This is the published `gpui-base` 0.7.3 crate (GPUI Kit's widgets, text
fields among them; Apache-2.0, `LICENSE-APACHE`) with the change below,
wired in through `[patch.crates-io]` in zcode's `Cargo.toml`. The unchanged
crate is the commit before this file's.

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

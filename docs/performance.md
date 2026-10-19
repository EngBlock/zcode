# Performance

Measured on an M4 Max with macOS 27.2 and a release build.

| | Content on screen | First frame produced |
|---|---|---|
| `zcode <project>` while zcode is running (the usual case) | | **~50–70 ms**; the CLI itself returns in ~50 ms |
| Cold launch, welcome window | **~230–240 ms** | ~200–210 ms (7 ms of it before `main()`) |
| Cold launch, project with 2 Swift tabs restored | | ~210–230 ms |
| Cold launch, ZKit with 4 Swift tabs restored in one pane / split over two panes | ~260 ms / ~275 ms | |
| *Reference:* minimal native AppKit app (one window, one line of text) | ~295 ms | ~270 ms |
| *Reference:* the same app with the open animation off | ~170 ms | ~150 ms |

Times run from process start. "Content on screen" is when the window
server's copy of the window first shows drawn content, which is what the user
sees. `zcode --startup-bench` polls the window's image from a background
thread and records the time after the capture returns, so slow captures count
against zcode. "First frame produced" is the next main-loop turn after the
app produced its first frame.

**Against Xcode:** timed from outside with `scripts/launch_timer.swift`,
which launches each app and polls the window server for its first window,
zcode's window appears in **~195–205 ms**. Xcode 27.0 takes **~930–990 ms**
(after a warm-up launch, with state restoration off; the warm-up itself took
~2 s). That makes zcode about 5× faster. `python3 scripts/e2e.py --compare-apps`
repeats the comparison and also times Zed when Zed isn't already running. Zed
allows only one instance, even with `--user-data-dir`, so it can't be cold
launched next to a running copy.

zcode shows its window ~55 ms sooner than a minimal native AppKit app and
~65 ms after that app's lower bound. `scripts/e2e.py` builds the reference app
(`scripts/appkit_floor.swift`), which measures itself the same way, and fails
if zcode is not faster.

* **Single instance.** zcode keeps running when its last window closes, as Xcode and other Mac document apps do. `zcode <path>` hands the path to the running instance over a Unix socket and exits, and the warm instance opens the project window in ~50–75 ms. `ZCODE_NEW_INSTANCE=1` forces a separate process.
* **Always warm.** `zcode --background` starts the resident instance with no window. zcode ▸ Open at Login registers it as a login item (the bundle's LaunchAgent, through SMAppService), and even the first open of the day takes the warm path.
* **The bundle costs ~15–25 ms more to launch.** `zcode.app/Contents/MacOS/zcode --startup-bench` is the same binary and the same startup, plus AppKit's own launch work for a bundled app: `finishLaunching` loads the Writing Tools UI library while it adds the text-input items to the Edit menu (seen with `sample`), which a bare binary without a bundle identifier skips. Three alternating rounds of 10 (load average 20–50): the bare binary's first pixels at 251.1, 260.1 and 259.4 ms, the bundle's at 287.3, 281.4 and 287.8 ms; on a calmer run (load average 18–25) 265.4, 266.8 and 262.8 ms against 281.6, 283.7 and 279.7 ms. The icon, the document types, the hardened runtime and the signature make no difference (each tried alone), nor do `NSAllowsWritingTools` or `NSMenuDoesNotAutomaticallyInsertWritingToolsItems` as `Info.plist` keys or defaults. The e2e suite times both in alternating rounds and holds the bundle to within 40 ms of the bare binary.
* Resident memory at first frame is ~102 MB for the welcome window and ~120 MB with a project open.
* **On a large app** (firefox-ios, 10,908 files; `scripts/bench.py`, `docs/scale.md`): the files are listed ~225 ms after `main()` (the scan takes ~75 ms in the background), an Open Quickly keystroke costs ~3 ms over every path, Find in Workspace reads every file in ~100 ms, a redraw with 5,000 results costs ~2 ms, typing in a 9,400-line file ~3 ms a keystroke, and zcode holds ~165–175 MB resident after opening and searching. The app's own files, which its index build can't compile without firefox-ios's bootstrap, get semantic highlighting through the build server (~8 s after opening one, once the index build has run; never with the compilation database alone).
* The first launch after each rebuild also includes macOS scanning the new binary.

* Two startup fixes make zcode faster than a bare GPUI Kit window
  (`cargo run --release --example gpui_baseline -- --quit --activate`, ~340–360
  ms):
  * zcode opens its windows hidden, turns off AppKit's window-open animation
    and then shows them. That removes ~110 ms that `makeKeyAndOrderFront`
    otherwise spends setting up the animation.
  * gpui-component's theme init listed every installed font (a ~35 ms
    synchronous round trip to `fontd`) to check two names. A one-function
    patch checks just those names (`vendor/gpui-component/ZCODE-PATCH.md`).
* What remains is mostly AppKit and the window server: ~50 ms of platform
  setup (Launch Services check-in, keyboard layout), ~25 ms creating the
  window, and the first frame. After the first frame, the app's activation
  (switching the menu bar, starting text input) keeps the main thread busy for
  ~60 ms before the next frame. The benchmark counts that time, although the
  first frame is already with the window server. `ZCODE_TRACE_STARTUP=1`
  prints the phases. `ZCODE_PAUSE_AT_START=<ms>` lets a profiler attach before
  startup begins.
* Language servers start the first time a file they serve opens (a
  project without web files never runs one), and the web's grammars'
  queries are compiled the first time a file in their language is
  highlighted, so launch costs the same with web apps in the project.
* sourcekit-lsp, scheme listing, simulator and device discovery, file
  scanning and git status all start after the window appears, on background
  threads. git status is read again after saves (debounced), git
  operations, window activation and changes in the git directory (FSEvents;
  see "Idle" below); change bars come from a diff of the editor's
  text run in the background 30 ms after the last keystroke, so typing
  latency doesn't change (median keystroke to frame in a 6,000-line file:
  5.3 ms with the change, 5.4 ms before). Devices are listed again when
  an Apple USB device attaches or detaches (IOKit's notifications, on a
  thread of their own), when the project's window becomes active and when
  the destination menu or Choose Destination opens, and otherwise once a
  minute while the window is active, only if its scheme builds for a device
  platform: an idle iOS project's window runs `devicectl list devices`
  (~60 ms of CPU, plus CoreDevice's work) once a minute instead of 12 times.
* Semantic tokens and inlay hints are asked for after typing pauses (300
  and 400 ms), a file's tokens whole and each pane's hints for its visible
  rows and a screen either side; a newer request cancels the one before and
  answers for text that has changed are dropped, so 30 keystrokes cost one
  semantic token request and a few hint requests. Typing in the middle of a
  6,000-line Swift file with both on: median keystroke to frame 5.0–5.1 ms,
  against 5.2–5.3 ms before (alternating runs, load average 4–8). A
  highlight layer follows an edit without sorting its ranges again.
* The terminal is parsed on its own thread (`alacritty_terminal`'s reader,
  which holds the grid's lock for at most 64 KB of output at a time) and
  repainted at most once a frame however fast output comes; a frame copies
  the visible cells out under the lock and shapes them after it. `yes |
  head -n 3000000` in a terminal takes ~3.6 s; typing in an editor pane
  meanwhile: median keystroke to frame 3.8 ms, as without it (a 27 MB
  `cat`: 0.2 s). Nothing of the terminal runs at startup: a shell starts
  on a background thread when its terminal is first shown, so a session
  restored with a terminal tab shows its first frame in ~209 ms (~228 ms
  with a Swift file's tab instead). Cold launch with it: median 247
  ms against 252 ms before (alternating runs, load average 8–11).
* Vim mode costs nothing while it's off: the editor has no keystroke
  interceptor or observer then, and the status bar follows Vim through an
  event an editor only emits with it on. On, insert mode passes keystrokes
  through after a check, and what `.` repeats is worked out once, at
  Escape. Typing in the middle of a 6,000-line Swift file, four
  alternating rounds each: median keystroke to frame 3.8–5.2 ms with Vim
  mode off and 5.0–5.2 ms in insert mode (load average 6–13). Cold launch,
  alternating against the build before (load average ~7): the welcome window's first
  pixels at 247.0 ms against 248.4 ms (median of 24), and ZKit with two
  Swift tabs restored at 226.3 ms against 224.5 ms to the first frame
  (median of 20), 225.9 ms with Vim mode on.
* The agent panel costs nothing until it's opened: no ACP code runs at
  startup, and a session that had it open shows it after the first frame
  without starting an agent; the agent starts (its command found on the
  login shell's `PATH`, read off the main thread once) when the first
  message is sent. Its process and the reading of what it sends are on
  threads of their own; each update re-lays out only the rows it changed
  in a virtual list. Cold launch, alternating runs against the build before: the
  welcome window's first pixels at 242.5 ms against 249.9 ms (median of 9).
  The binary grows by 1.9 MB (the protocol's types).
* The MCP server costs nothing until a client connects: the socket was
  already there for `zcode <path>`, and a connection's reading and writing
  are on threads of their own. `zcode mcp` itself is a relay that checks
  its first argument before anything else starts, so it starts in a few
  milliseconds. Cold launch, alternating against the build before (load average
  ~9.6): the welcome window's first pixels at 254.8 ms against 254.6 ms
  (median of 12).
* Predictive completion costs nothing while it's off (the default): no
  subscriptions, tasks or threads, one comparison per edit, and the editor
  has no observer without a prediction to show. On, typing only resets a
  timer; a request is built once typing pauses (a slice of the file around
  the cursor and a few coalesced recent edits) and sent on a thread of its
  own, its answer streamed back to the main thread a piece at a time. The
  HTTP client is made on the first request. Typing in a file with
  predictions on: median keystroke to frame 2.0 ms, against 3.0 ms with
  them off (the e2e check's run). Cold launch, three alternating rounds of
  15 against the build before (load average 7–10): the welcome window's first
  pixels at 263.8, 257.9 and 260.7 ms against 258.4, 259.8 and 263.8 ms.
  The binary grows by 1.4 MB (`ureq` and `native-tls`).
* Profile costs nothing until it's used: Instruments' templates are listed
  (`xctrace list templates`, on a thread of its own) the first time
  Profile asks for one, a recording's output streams like a run's, and the
  summary's export is read off the main thread. Cold launch, three
  alternating rounds of 15 against the build before (load average 10–25): the
  welcome window's first pixels at 236.6, 234.9 and 237.7 ms against
  231.8, 240.4 and 235.3 ms.
* Archive and the Organizer cost nothing until used: the Organizer reads
  the Archives folder on a thread of its own when it opens (and after
  zcode archives, exports or deletes), and Distribute App's export,
  notarization and stapling stream like a build. Cold launch, three
  alternating rounds of 12 against the build before (load average ~8): the
  welcome window's first pixels at 237.1, 228.5 and 237.2 ms against
  230.5, 234.6 and 230.1 ms.
* Asset catalogs cost nothing until one is opened: the catalog is read on
  a thread of its own (3,000 image sets in ~2.5 s in the e2e check, the
  window answering throughout), and thumbnails are decoded in the
  background at most four at a time, only for the wells and tiles on
  screen (the outline and a folder's tiles are virtual lists), scaled to
  192 pixels and kept in a cache of at most 64 MB that drops the least
  recently shown (their textures freed too). Edits run in the background.
  Cold launch, three alternating rounds of 10 against the build before (load
  average 10–14): the welcome window's first pixels at 231.3, 224.7 and
  219.2 ms against 227.4, 229.2 and 223.7 ms. The binary grows by 1.5 MB
  (the PDF and image decoding zcode now calls; the crates were already
  linked).
* SwiftUI previews cost nothing at startup: a Swift file is searched for
  previews when it opens and 300 ms after typing stops, in the background
  (files that don't mention `#Preview` or `PreviewProvider` aren't
  parsed), and the gutter reads what was found. Show in Xcode Canvas waits
  for Xcode on a thread of its own. Cold launch, three alternating rounds
  of 10 against the build before (load average 12–16): the welcome window's first
  pixels at 250.3, 243.8 and 246.1 ms against 245.3, 249.2 and 244.6 ms.
  The binary grows by 0.2 MB.
* Packaging costs nothing at startup: the bundle is found from the
  executable's path the first time something asks (the update check, the
  control channel), the login item's status is asked of launchd on a
  thread of its own 1.5 s after launch, and the first update check runs a
  minute after launch and then once a day, on a thread of its own; the
  three menu items are three more entries in the menu built at startup.
  `Info.plist` and the icon are read by AppKit and LaunchServices, not by
  zcode. Finder's file URLs arrive through `on_open_urls` into a channel
  the first window's code drains, so a document opened at launch doesn't
  flash the welcome window. Cold launch, three alternating rounds of 10
  against a build of the commit before it (load average 20–50): the welcome
  window's first pixels at 251.1, 260.1 and 259.4 ms against 269.9, 269.0
  and 261.8 ms. The binary grows by 0.05 MB (`objc2-service-management`).
* Settings cost nothing measurable at startup: the user's settings,
  keymap and the theme they name are read and parsed on a thread of their
  own started at the top of `main()`, and the first window takes what it
  read (~0.06 ms on the main thread); with no files it's one failed
  `open` each. A project's `.zcode/settings.json` is read when its window
  opens, and every file is read again when FSEvents says it changed
  (see "Idle" below). Cold launch, alternating runs against the build before (load average
  9–17): the welcome window's first pixels at 253.2 ms with no settings
  files and 255.0 ms with settings, a custom theme, a font and key
  bindings, against 259.0 ms before (median of 20); ZKit at 262.4 /
  265.2 ms against 262.7 ms (median of 12).
* Swift highlighting uses a compact tree-sitter query
  (`src/languages/swift-highlights.scm`) that compiles in ~11 ms. The upstream
  query takes ~185 ms (`cargo run --release --example query_bench`). A
  language's queries are compiled once and shared by every editor (a patch
  to gpui-component's highlighter; upstream compiles them for each editor,
  and an injected language's on every parse): opening 30 firefox-ios Swift
  files costs 1.0 s of CPU against 1.4 s before. When a project opens,
  its restored tabs' languages are compiled on a background thread while
  the window is created, so the shown tab's highlighter no longer compiles
  them on the main thread: ZKit with four Swift tabs restored shows its
  first frame at 201 ms against 218 ms before (median of 15,
  alternating, load average 9–20); the welcome window is unchanged
  (232.5 ms against 233.9 ms to its first pixels, median of 15).
* Big files: an edit costs what it touches. The gutter finds a row's fold
  by binary search, not a scan of every fold (Page Down ×5 in a 15 MB JSON
  file: 15–19 ms of work against 23–31 ms before); an edit's fold
  candidates come from the syntax tree's nodes around it, not from every
  node at the top level; and a language server is sent the editor's own
  edits (`EditJournal`, recorded as each edit is applied), checked against
  the text, with a rope comparison that skips the chunks both texts share
  when they don't add up. Typing near the end of a 50,000-line Swift file:
  1.9–2.9 ms of work per keystroke against 6.7–7.8 ms before
  (`scripts/bench.py`'s `typing_huge_ms` and `pagedown_json_ms`;
  `docs/scale.md`).
* Long lines are laid out only where they're seen. A line over 4 KB (a
  minified script, a JSON or log record) gets the x of a checkpoint every
  ~1 KB, summed from the editor font's width for each character and
  shifted rather than measured again after an edit; a frame shapes only
  the 256-byte tiles across the text area and around the caret (with soft
  wrap, the rows on screen and around it) and styles only those bytes, and
  carets, selections, clicks and vertical movement measure from the
  checkpoints. Soft wrap wraps such a line by the same widths and re-wraps
  an edit only until its rows meet the old ones. Lines over 10,000 bytes
  get no syntax colors (VS Code stops at 20,000), and long lines leave out
  inlay hints. In a 3.3 MB one-line minified script (alternating launches
  against the build before, load average 3–5): typing 3.2 ms of work a keystroke
  at its start and 7.0 ms at its end against 37 and 63 ms, five Page Downs
  12.8 ms against 61.5 ms, turning soft wrap on 58 ms to the frame on
  screen against 5.5 s, typing at the end with it on 7.0 ms against 2.8 s,
  and the same file as plain text adds 14.5 MB of resident memory against
  225 MB. LSP positions and the status bar's column come from the rope's
  UTF-16 and character counts rather than a walk of the line
  (`docs/scale.md`, Performance batch 2).
* **Idle:** a window with nothing to draw doesn't wake. GPUI's macOS
  backend kept its display link running while a window was visible, waking
  the main thread on every refresh (120 Hz here) to find nothing to draw; a
  patch (`vendor/gpui-pre-macos/ZCODE-PATCH.md`) pauses it after four frame
  requests with nothing asked for, and GPUI's frame waker (any notify,
  refresh, animation frame) starts it again with a frame at once (a
  keystroke after a pause is on screen in 6.8 ms against 7.4–7.7 ms). Open
  files, settings files, the git directory and the build server's project
  files are looked at when FSEvents says something under them changed
  (`src/fs_watch.rs`, one stream on a thread of its own started 1.5 s after
  the window), not every 1–3 s; a check polls at its old interval only where
  the stream can't cover its files, and every 30 s anyway. The status bar's
  memory figure is the 30 s idle baseline (the last sample flipped after
  each redraw and so redrew the window). Over 60 s idle, against the build before
  (alternating launches, load 1.3–2.1): the welcome window 11 ms of CPU
  against 429–566 ms and 41 interrupt wakeups against 8,700, ZKit 35–50 ms
  against 722–761 ms, HelloZ 36–42 ms against 734–762 ms; with a caret
  blinking, 24 wakeups a second against 161 (the e2e check, its control
  channel included). A file changed outside
  zcode is read again in ~0.2 s (up to 3 s before), settings.json in ~0.2 s.
* **Large files:** a file over 2 MB, or over 256 KB and mostly lines past
  10,000 bytes (minified), opens with its language (comments, brackets,
  indentation, language servers) but without a tree-sitter tree, whose
  memory is 25–60 times the file: no syntax colors, folds or string and
  comment context, which the status bar says ("Large file" / "Minified: no
  syntax highlighting"). Soft wrap's index keeps lines that are one row as
  runs of their count, so it costs what the wrapped lines do (~110 bytes a
  line before). Heap added by opening a file, against the build before: a 15 MB
  JSON file 22 MB against 514 MB, a 3.3 MB minified script 5 MB against
  194 MB, a 1.2 MB Swift file (which keeps its tree) 55 MB against 61 MB.
  Files up to 64 MB open (16 MB before; a 60 MB log adds 88 MB and types at
  3 ms of work a key); past 16 MB a file goes to no language server and
  gets no change bars.
* **Tabs:** an editor in a tab no pane shows lets go of its layout, and a
  layout's lines share their whitespace marks instead of each holding a
  copy (a line's layout was 9.1 KB). 40 firefox-ios Swift tabs add 15 MB of
  heap against 32 MB before, about half of it their syntax trees.
* **About memory:** while a GPUI window is frontmost and redrawing (for
  example while the caret blinks), macOS attributes about 220 MB of transient
  compositor/GPU memory to the process (the "Owned physical footprint
  (graphics)" category in `footprint`). That memory is released within a
  second once redraws stop or the window goes to the background, and a bare
  GPUI window shows exactly the same behavior. The Performance panel reports
  both the current footprint and a 30-second idle baseline. zcode itself
  holds about 85 MB.

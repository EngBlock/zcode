# zcode

A fast, Zed-inspired editor for Apple-platform development, written in Rust on
[GPUI](https://gpui.rs) (Zed's GPU UI framework) via
[GPUI Kit](https://gpui-kit.com). It edits, builds, runs, tests and debugs
Xcode projects and Swift packages without opening Xcode. It uses the
toolchain Xcode installs: `xcodebuild`, `simctl`, `sourcekit-lsp` and
`lldb-dap`.

```
cargo build --release
./target/release/zcode path/to/App.xcodeproj   # or .xcworkspace, Package.swift, a folder, a file
```

Requirements: macOS with Xcode installed (`xcode-select -p` pointing at it), Rust 1.90+.

## What works

| Area | Feature | Keys (Xcode-compatible) |
|---|---|---|
| **Projects** | Opens `.xcodeproj`, `.xcworkspace`, Swift packages and plain folders. Includes a welcome window with recent projects and per-project session restore (tabs, scheme, destination, breakpoints, layout). | ⌘O |
| | Project settings: click the project in the navigator to edit the project and its targets (General fields such as bundle ID, version, deployment targets and team, plus every build setting per configuration). Edits are written straight to `project.pbxproj` and keep its formatting. The panel can also open `project.pbxproj` as text or open the project in Xcode. | — |
| **Editing** | Rope-based editor with tree-sitter highlighting (Swift, C/ObjC, C++, JSON, YAML, Markdown…), multiple cursors, folding, indent guides and find/replace. | ⌘F, ⌥⌘F |
| | Comment selection, duplicate, move line, select next occurrence, soft wrap and font size. | ⌘/, ⌘D, ⌥⌘[ ⌥⌘], ⌥⌘E, ⌃⇧⌘L, ⌘+ ⌘- ⌃⌘0 |
| | Xcode-style `<#placeholder#>` snippets from completions, with Tab or ⌃/ jumping to the next placeholder. | Tab, ⌃/, ⌃⇧/ |
| **Language intelligence** (sourcekit-lsp) | Completion, live diagnostics, Quick Help on hover, Jump to Definition (also ⌘-click), Find References and Rename across files. | ⌃Space, ⌃⌘J, ⌃⇧⌘F, F2 |
| | Edit All in Scope, Format File / Re-Indent, code actions and fix-its, Document Items outline, and workspace symbols in Open Quickly. | ⌃⌘E, ⌃⇧I / ⌃I, ⌃⌥⌘F, ⌃6 |
| **Xcode project indexing** | zcode pulls a compilation database out of `xcodebuild` output, the job xcode-build-server does, so cross-file features work for `.xcodeproj` targets. When a project has never been built, a low-priority background "index build" produces it. Saving source files refreshes the index, so rename and references stay current. | — |
| **Navigation** | Open Quickly (fuzzy file and symbol search, `file:line`), Quick Actions command palette, back/forward history, jump to line, counterpart (.h ↔ .m), reveal in navigator and tab switching. | ⇧⌘O, ⇧⌘A, ⌃⌘← ⌃⌘→, ⌘L, ⌃⌘↑, ⇧⌘J, ⇧⌘[ ⇧⌘] |
| | Group-based projects (no file-system-synchronized folders) stay consistent as Xcode keeps them: New File, Rename (a name with `/` moves the item), Move to Trash and Add Files… add, update and remove file references, groups and build files in `project.pbxproj` (Sources for code, Resources for resources), with Xcode-style IDs and the rest of the file left byte for byte. New files join the targets that build their neighbors. | ⌥⌘A |
| | File inspector: name, type, location, full path and Target Membership checkboxes for the selected file. | ⌥⌘1 |
| **Navigators** | Project (file tree with New File / Add Files / Rename / Move to Trash / Show File Inspector / Show in Finder), Source Control (git status), Symbols, Find (find in workspace with case, word and regex options), Issues (live + build), Tests, Debug (threads and frames), Breakpoints and Reports (build/run/test history with full logs). | ⌘1 … ⌘9 |
| **Build & run** | Scheme, configuration and run destination pickers. Destinations are My Mac plus every available simulator, filtered by the scheme's `SUPPORTED_PLATFORMS`. | ⌃0, ⌃⇧0 |
| | Build, Run, Test, Stop and Clean. Files are saved first. | ⌘B, ⌘R, ⌘U, ⌘., ⇧⌘K |
| | Edit Scheme for the Run and Test actions: build configuration, Debug executable, arguments passed on launch and environment variables (each with its checkbox; `$(BUILD_SETTING)`s expand), working directory, launching without state restoration, and the Address, Thread and Undefined Behavior Sanitizers, Main Thread Checker, Malloc Scribble, Guard Edges and Stack Logging, and Zombie Objects. Changes are written in place to the scheme's `.xcscheme` (shared or the user's), the rest of the file left byte for byte; Swift packages keep them in zcode's session. Run… and Test… open the editor with a button that starts the action. Product ▸ Scheme also has Next and Previous Scheme. | ⌘<, ⌥⌘R, ⌥⌘U, ⌃⌘] ⌃⌘[ |
| | The scheme's options reach every launch: simulator apps (`simctl launch` with `SIMCTL_CHILD_` variables), Mac apps, package products and lldb-dap; sanitizers reach `xcodebuild` and `swift build`/`swift test`, tests build with the Test action's configuration, and the Test action's environment reaches `swift test` and the in-simulator runner. The toolbar's build configuration is the Run action's. Mac executables and package products run on a pseudo-terminal, as in Xcode's console, so their output streams as it is printed. | — |
| | Running on a simulator boots it, opens Simulator.app, installs and launches the app, and streams the app's stdout/stderr into the console. Mac executables and Swift packages run directly. | — |
| | Build output streams into the debug area as a condensed Xcode-style activity log, or as the raw `xcodebuild` log. Errors and warnings appear inline, in the Issue navigator and with next/previous issue keys. | ⇧⌘Y, ⇧⌘C, ⌘K, ⌘' ⌘" |
| | Test results from XCTest and Swift Testing populate the Test navigator, and Swift Testing failures appear as issues at the failing line. On a simulator, unit tests run inside the host app by injecting the test bundle (XCTest's own hosted-test mechanism). That is faster than `xcodebuild test` and doesn't depend on its test-runner service. | ⌘U |
| **Testing** | Test diamonds in the editor gutter on XCTest classes and `test…` methods, Swift Testing `@Suite` types and `@Test` functions (found in source as you type, Objective-C XCTest too). They show the last result; clicking one runs just that test or class. | — |
| | Test Selected Items runs the test (or class) at the cursor or selected in the Test navigator; Test Again repeats the last test run; Re-run Failed Tests runs what failed last; Clear Test Results. | ⌃⌥⌘U, ⌃⌥⌘G, ⌃⌥⌘K |
| | The Test navigator lists targets, classes, suites and tests from source with their results; each row runs on its play button or context menu. Results of tests a run doesn't include stay, as in Xcode. | ⌘6 |
| | Test plans: the scheme's `.xctestplan`s in Product ▸ Choose Test Plan… and the Test navigator; ⌘U and Test Again run the chosen plan, and tests it leaves out are dimmed. | — |
| | Every runner gets the selection: `xcodebuild test -only-testing:`/`-testPlan`, `swift test --filter`, and in the simulator an XCTest scope for the injected bundle (which also selects Swift Testing tests), honoring the plan's skipped and selected tests, environment and arguments. | — |
| **Debugging** (lldb-dap) | Breakpoints (toggle, deactivate all, persisted per project). ⌘R runs under the debugger whenever breakpoints are active, as in Xcode. Simulator apps launch with `--wait-for-debugger` and lldb attaches to them. | ⌘\\, ⌘Y |
| | Pause/continue, step over/into/out, the stopped line highlighted in green, threads and frames in the Debug navigator, and an expandable Variables view next to the console. | ⌃⌘Y, F6, F7, F8 |
| **Monitoring** | Memory is sampled every 2 s for the life of the app. The status bar shows zcode's footprint and the language server's (including its `SourceKitService` XPC helpers), and turns orange above 1.5 GB idle for zcode or 4 GB for the language server. View ▸ Performance shows startup timings, the current and 30 s idle-baseline footprint, peak, and a history graph. `--perf-log <file>` (or `ZCODE_PERF_LOG`) appends every sample as a JSON line for long-term tracking. `zcode --startup-bench` prints startup and memory as JSON and quits. | — |

The full Xcode 27 menu and shortcut inventory this was matched against is in
[`docs/xcode-inventory.md`](docs/xcode-inventory.md), and remaining gaps are in
[`docs/parity.md`](docs/parity.md).

## Performance

Measured on an M4 Max with macOS 27.2 and a release build.

| | Content on screen | First frame produced |
|---|---|---|
| `zcode <project>` while zcode is running (the usual case) | | **~50–70 ms**; the CLI itself returns in ~50 ms |
| Cold launch, welcome window | **~230–240 ms** | ~200–210 ms (7 ms of it before `main()`) |
| Cold launch, project with 2 Swift tabs restored | | ~210–230 ms |
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
* **Always warm.** `zcode --background` starts the resident instance with no window. Run it as a login item (System Settings ▸ General ▸ Login Items, or a LaunchAgent), and even the first open of the day takes the warm path.
* Resident memory at first frame is ~102 MB for the welcome window and ~120 MB with a project open.
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
* sourcekit-lsp, scheme listing, simulator discovery, file scanning and git
  status all start after the window appears, on background threads.
* Swift highlighting uses a compact tree-sitter query
  (`src/languages/swift-highlights.scm`) that compiles in ~11 ms. The upstream
  query takes ~185 ms, and the editor pays that cost for every Swift file it
  opens (`cargo run --release --example query_bench`).
* **About memory:** while a GPUI window is frontmost and redrawing (for
  example while the caret blinks), macOS attributes about 220 MB of transient
  compositor/GPU memory to the process (the "Owned physical footprint
  (graphics)" category in `footprint`). That memory is released within a
  second once redraws stop or the window goes to the background, and a bare
  GPUI window shows exactly the same behavior. The Performance panel reports
  both the current footprint and a 30-second idle baseline. zcode itself
  holds about 85 MB.

## How it fits together

```
src/
  app.rs            startup, windows, global commands, --startup-bench
  actions.rs        commands, Xcode key bindings, menu bar
  workspace/        project window: buffers, navigators, panels, pickers, editor commands
  lsp/              JSON-RPC client, document sync store, editor provider adapters
  build/            xcodebuild/swift/simctl queries, streaming runner, log/issue/test parsing,
                    compilation-database extraction
  testing/          tests found in source, test plans, test selections per runner, results → tests
  scheme/           Edit Scheme options: .xcscheme read and edited in place, what launches get
  debugger/         DAP client and debugger model (lldb-dap)
  project.rs        project detection and gitignore-aware scanning
  xcodeproj.rs      project.pbxproj parser and in-place editor (build settings)
  xcodeproj/        structural edits: files, groups, build files, target membership
  search.rs         parallel find-in-workspace
  perf.rs           startup timing and memory sampling (proc_pid_rusage)
  debug.rs          scriptable UI control channel for tests (ZCODE_DEBUG_DIR)
vendor/
  gpui-component/   gpui-component 0.7.3 plus small patches (ZCODE-PATCH.md)
scripts/
  e2e.py            end-to-end UI tests
  appkit_floor.swift  minimal native AppKit app, the cold-launch reference
```

The editor engine (rope, layout, tree-sitter highlighting, completion and
hover UI) comes from `gpui-component`, which is Apache-2.0. zcode vendors it
with a startup patch and a popover padding fix, both described in
`vendor/gpui-component/ZCODE-PATCH.md`.
zcode doesn't use any GPL code from Zed.

## Testing

```
cargo test                          # unit tests
cargo test -- --ignored             # needs Xcode: sourcekit-lsp, lldb-dap, and xcodebuild on an edited project
cargo build --release && python3 scripts/e2e.py [--simulator] [--compare-apps]   # drives the real UI
```

`scripts/e2e.py` launches zcode with `ZCODE_DEBUG_DIR` set and checks against
a temporary copy of `samples/` taken from git `HEAD`, so local edits and
saved breakpoints can't leak in. It covers:
* Open Quickly, definition, completion with placeholders, comment and undo, Document Items, find and references
* project settings: a General field edited and restored, with
  `project.pbxproj` checked byte for byte
* a group-based project (`samples/HelloClassic`): New File in a new folder,
  Target Membership on and off, rename and move, Move to Trash, Add Files,
  then a build of the edited project; and in the synchronized HelloZ, the same
  operations leave `project.pbxproj` untouched
* the build-error → issue flow
* test diamonds: one Swift Testing test from its diamond (`xcodebuild
  -only-testing`), Test Selected Items at the cursor, Test Again, a whole
  suite, and the Greeting test plan; in a Swift package a test and an XCTest
  class (`swift test --filter`) and Re-run Failed Tests after a fix
* run, test and debug for a Swift package
* Edit Scheme: arguments, environment variables and diagnostics written into
  HelloClassic's shared scheme (checked line by line, and byte for byte once
  undone), a Mac app run that receives them, the Test action's configuration;
  Run… and Test… on a Swift package with options kept in the session (also
  under lldb-dap, and after a relaunch); a project without scheme files
* cold launch: first frame before the minimal native AppKit app and under 250 ms (medians of 5); opening a project through an already-running instance (< 150 ms)
* every Xcode 27 default shortcut for the MVP commands (from
  `docs/xcode-inventory.md`) is bound to the matching zcode command in the live
  keymap
* memory monitoring: sampler history, language-server and `SourceKitService`
  memory, the status-bar label and the `--perf-log` file
* with `--simulator`: unit tests, one test and one suite from their diamonds,
  the Smoke test plan, and run (with the scheme's arguments and environment)
  and debug on an iOS simulator

The control channel also takes window screenshots
(`screenshot <path>`), which doesn't need Screen Recording permission.

## Known limitations

* Project settings cover build settings and the General fields. Adding or
  removing targets, configurations, packages or capabilities still needs
  Xcode.
* In file-system-synchronized folders (the Xcode 16+ default) files belong to
  the folder's targets; zcode doesn't edit their per-file exceptions, so the
  File inspector shows that membership read-only. Trashing a whole
  synchronized folder leaves its group in the project.
* Structural edits don't create variant groups (localized `.lproj` folders
  are added as folder references) or `XCVersionGroup`s (`.xcdatamodeld` is
  added as a plain file reference), and Headers-phase visibility (Public,
  Private) isn't editable. The navigator has no drag and drop: move items by
  renaming them to a path.
* No Interface Builder, asset-catalog editor, SwiftUI canvas, Instruments or
  physical-device deployment.
* Edit Scheme covers the Run and Test actions only (not Build, Profile,
  Analyze or Archive, nor pre- and post-actions), and doesn't create,
  duplicate, share or delete schemes. It has no App Language or Region,
  location simulation, StoreKit or "Pause on issues" options. Main Thread
  Checker applies under the debugger, as in Xcode.
* A scheme that runs test plans takes its test arguments, environment and
  diagnostics from the plan (as `xcodebuild` does), and plans can't be edited
  yet. `swift test` doesn't pass launch arguments to tests. Schemes
  `xcodebuild` makes up for projects without scheme files keep their options
  in zcode's session instead of a new user scheme, so Xcode doesn't see them;
  their test environment reaches `xcodebuild test` (as `TEST_RUNNER_`
  variables) but their test launch arguments only reach the in-simulator
  runner.
* Tests are found by reading source, so tests that only exist at run time
  (XCTest methods added dynamically, tests from macros other than `@Test` and
  `@Suite`) have no diamond; their results still appear under Other Results
  in the Test navigator. Swift Testing's console names a test but not its
  suite, so where names repeat across suites zcode runs Swift Testing one
  test at a time (`swift test --no-parallel`; in Xcode projects XCTest
  already does) to tell results apart.
* A test plan with several configurations runs each one in the simulator,
  but the Test navigator keeps one result per test (the last configuration's).
  Test plans can be chosen, not edited.
* Hostless (library) test bundles on a simulator get the same selection, but
  that path is untested here: every sample's tests are hosted.
* UI test bundles are skipped by the in-simulator unit-test runner and still
  need `xcodebuild test` (running only UI tests from a diamond uses it). On this machine that fails ("The test runner hung
  before establishing connection") because of the simulator runtime, with
  either installed Xcode, independent of zcode.

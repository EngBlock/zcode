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
| **Editing** | Rope-based editor with tree-sitter highlighting (Swift, C/ObjC, C++, JSON, YAML, Markdown…), multiple cursors, folding, indent guides and find/replace. | ⌘F, ⌥⌘F |
| | Comment selection, duplicate, move line, select next occurrence, soft wrap and font size. | ⌘/, ⌘D, ⌥⌘[ ⌥⌘], ⌥⌘E, ⌃⇧⌘L, ⌘+ ⌘- ⌃⌘0 |
| | Xcode-style `<#placeholder#>` snippets from completions, with Tab or ⌃/ jumping to the next placeholder. | Tab, ⌃/, ⌃⇧/ |
| **Language intelligence** (sourcekit-lsp) | Completion, live diagnostics, Quick Help on hover, Jump to Definition (also ⌘-click), Find References and Rename across files. | ⌃Space, ⌃⌘J, ⌃⇧⌘F, F2 |
| | Edit All in Scope, Format File / Re-Indent, code actions and fix-its, Document Items outline, and workspace symbols in Open Quickly. | ⌃⌘E, ⌃⇧I / ⌃I, ⌃⌥⌘F, ⌃6 |
| **Xcode project indexing** | zcode pulls a compilation database out of `xcodebuild` output, the job xcode-build-server does, so cross-file features work for `.xcodeproj` targets. When a project has never been built, a low-priority background "index build" produces it. Saving source files refreshes the index, so rename and references stay current. | — |
| **Navigation** | Open Quickly (fuzzy file and symbol search, `file:line`), Quick Actions command palette, back/forward history, jump to line, counterpart (.h ↔ .m), reveal in navigator and tab switching. | ⇧⌘O, ⇧⌘A, ⌃⌘← ⌃⌘→, ⌘L, ⌃⌘↑, ⇧⌘J, ⇧⌘[ ⇧⌘] |
| **Navigators** | Project (file tree with New File / Rename / Move to Trash / Show in Finder), Source Control (git status), Symbols, Find (find in workspace with case, word and regex options), Issues (live + build), Tests, Debug (threads and frames), Breakpoints and Reports (build/run/test history with full logs). | ⌘1 … ⌘9 |
| **Build & run** | Scheme, configuration and run destination pickers. Destinations are My Mac plus every available simulator, filtered by the scheme's `SUPPORTED_PLATFORMS`. | ⌃0, ⌃⇧0 |
| | Build, Run, Test, Stop and Clean. Files are saved first. | ⌘B, ⌘R, ⌘U, ⌘., ⇧⌘K |
| | Running on a simulator boots it, opens Simulator.app, installs and launches the app, and streams the app's stdout/stderr into the console. Mac executables and Swift packages run directly. | — |
| | Build output streams into the debug area as a condensed Xcode-style activity log, or as the raw `xcodebuild` log. Errors and warnings appear inline, in the Issue navigator and with next/previous issue keys. | ⇧⌘Y, ⇧⌘C, ⌘K, ⌘' ⌘" |
| | Test results from XCTest and Swift Testing populate the Test navigator. | ⌘U |
| **Debugging** (lldb-dap) | Breakpoints (toggle, deactivate all, persisted per project). ⌘R runs under the debugger whenever breakpoints are active, as in Xcode. Simulator apps launch with `--wait-for-debugger` and lldb attaches to them. | ⌘\\, ⌘Y |
| | Pause/continue, step over/into/out, the stopped line highlighted in green, threads and frames in the Debug navigator, and an expandable Variables view next to the console. | ⌃⌘Y, F6, F7, F8 |
| **Monitoring** | The status bar shows zcode's memory and the language server's memory (including `SourceKitService` XPC helpers). View ▸ Performance shows startup timings and memory history. `zcode --startup-bench` prints the same data as JSON and quits. | — |

The full Xcode 27 menu and shortcut inventory this was matched against is in
[`docs/xcode-inventory.md`](docs/xcode-inventory.md), and remaining gaps are in
[`docs/parity.md`](docs/parity.md).

## Performance

Measured on an M4 Max with macOS 27.2 and a release build. These are warm launches; the first launch after
every rebuild includes macOS scanning the new binary.

| | Welcome window | Project with 2 Swift tabs restored |
|---|---|---|
| exec → `main()` | ~8 ms | ~10 ms |
| `main()` → first frame | ~355 ms | ~370–410 ms |
| Resident memory at first frame | ~106 MB | ~124 MB |

* zcode adds close to nothing on top of GPUI. A bare GPUI Kit window
  (`cargo run --release --example gpui_baseline -- --quit --activate`) also
  takes ~340–360 ms to its first frame. Most of that is framework
  initialization (~55 ms platform setup, ~40 ms component init, ~150 ms
  creating the Metal window).
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
  debugger/         DAP client and debugger model (lldb-dap)
  project.rs        project detection and gitignore-aware scanning
  search.rs         parallel find-in-workspace
  perf.rs           startup timing and memory sampling (proc_pid_rusage)
  debug.rs          scriptable UI control channel for tests (ZCODE_DEBUG_DIR)
```

The editor engine (rope, layout, tree-sitter highlighting, completion and
hover UI) comes from `gpui-component`, which is Apache-2.0. zcode doesn't use
any GPL code from Zed.

## Testing

```
cargo test                          # unit tests
cargo test -- --ignored             # needs Xcode: sourcekit-lsp + lldb-dap integration tests
cargo build --release && python3 scripts/e2e.py [--simulator]   # drives the real UI
```

`scripts/e2e.py` launches zcode with `ZCODE_DEBUG_DIR` set and checks against
the sample projects in `../samples`:
* Open Quickly, definition, completion with placeholders, comment and undo, Document Items, find and references
* the build-error → issue flow
* run, test and debug for a Swift package
* with `--simulator`: run and debug on an iOS simulator

The control channel also takes window screenshots
(`screenshot <path>`), which doesn't need Screen Recording permission.

## Known limitations

* Xcode projects are read through `xcodebuild`; editing `.pbxproj` (adding
  targets or files to groups) isn't supported. Projects that use
  file-system-synchronized groups (the Xcode 16+ default) pick up new files
  automatically.
* No Interface Builder, asset-catalog editor, SwiftUI canvas, Instruments or
  physical-device deployment.
* In this environment the simulator's XCTest runner couldn't connect
  (`xcodebuild test` hangs identically outside zcode). zcode reports the
  runner's failure in the Issue navigator. Swift package tests run normally.

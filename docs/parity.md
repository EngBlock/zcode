# Xcode parity

Status of each item from the MVP list in [`xcode-inventory.md`](xcode-inventory.md),
which was extracted from Xcode 27's own command definitions.

✅ done · 🟡 partial · ⬜ not yet

## P0: edit, build, run on a simulator

| Item | Status | Notes |
|---|---|---|
| Open a folder / `.xcodeproj` / `.xcworkspace` / `Package.swift`, project navigator | ✅ | Context menu: New File, Rename, Move to Trash, Show in Finder |
| Source editor with tabs, highlighting, Save/Save All, undo, find/replace, ⌘/, ⌘[ ⌘], ⌥⌘[ ⌥⌘], ⌘L | ✅ | |
| sourcekit-lsp: completion, diagnostics, hover, ⌃⌘J, references, ⌃6 | ✅ | `.xcodeproj` indexing through a compilation database pulled from `xcodebuild` output (no xcode-build-server needed) |
| Open Quickly (⇧⌘O), Quick Actions (⇧⌘A) | ✅ | Files and workspace symbols, `file:line` |
| Scheme and destination pickers | ✅ | `xcodebuild -list`, `simctl list`, filtered by `SUPPORTED_PLATFORMS`; Debug/Release |
| Build, Run, Stop, Clean (⌘B ⌘R ⌘. ⇧⌘K) | ✅ | Simulator boot, install and launch; Mac executables; Swift packages |
| Issue navigator, ⌘' / ⌘" | ✅ | Live and build issues, de-duplicated, shown inline |
| Console / debug area (⇧⌘Y, ⌘K) | ✅ | App stdout/stderr via `simctl launch --console-pty` |
| Find in Workspace (⇧⌘F) | ✅ | Parallel and gitignore-aware; case, whole word and regex options |

## P1: things people use every day

| Item | Status | Notes |
|---|---|---|
| Debugging with lldb-dap: breakpoints (⌘\\, ⌘Y), pause/continue (⌃⌘Y), step (F6/F7/F8), Debug navigator, Variables view, Breakpoints navigator | ✅ | Attaches to simulator apps and launches Mac executables; breakpoints are persisted. No LLDB REPL or conditional breakpoints yet |
| Test (⌘U), Tests navigator | 🟡 | XCTest and Swift Testing results are parsed. No run-single-test or inline failure markers |
| Navigation history ⌃⌘← ⌃⌘→, counterpart ⌃⌘↑, reveal ⇧⌘J | ✅ | |
| Rename, Edit All in Scope (⌃⌘E), fix-its (⌃⌥⌘F) | ✅ | Rename opens affected files as unsaved tabs for review; the index refreshes in the background after save |
| Re-indent (⌃I), Format File (⌃⇧I) | ✅ | Via sourcekit-lsp (swift-format) |
| Split panes / assistant | ⬜ | |
| Git: change gutter, blame, commit | 🟡 | Read-only Source Control navigator (status and branch) |
| Swift packages: build, run, test | ✅ | Resolve/Update are handled implicitly by `swift build` |
| Build configuration picker | ✅ | Run options dialog (arguments, environment) not yet |
| Physical devices (devicectl) | ⬜ | |

## P2

| Item | Status |
|---|---|
| Call/type hierarchy | ⬜ |
| Folding, multi-cursor | ✅ (from the editor engine) |
| Minimap, Vim mode | ⬜ |
| Reports navigator (log history) | ✅ |
| Profile (⌘I) with xctrace | ⬜ |
| Archive/export | ⬜ |
| SwiftUI Previews | ⬜ (needs private preview infrastructure) |

## Beyond Xcode (from Zed)

* A fuzzy command palette with every command and its shortcut (⇧⌘A / ⇧⌘P).
* Live memory monitoring for both the editor and the language server, a
  startup benchmark mode, and an idle-baseline memory view.
* A scriptable UI control channel (`ZCODE_DEBUG_DIR`) for automated UI tests.

# Roadmap

How zcode keeps closing the gap with Xcode (workflow parity) and Zed (editor
quality). Item status lives in [`parity.md`](parity.md); this file is the
plan and the order of work.

**Rule:** every phase removes a reason to open Xcode or Zed without giving up
what zcode already guarantees: < 250 ms cold launch, the warm-open path, the
memory budgets, and e2e coverage (`scripts/e2e.py`) for every feature.

## Phase 0: land in-flight work

1. Commit the project-settings / `project.pbxproj` editing work; reconcile the
   README (feature table vs. "Known limitations") and `parity.md`. ✅
2. Structural `.pbxproj` edits: add file to group + target, target
   membership, remove references on trash/rename. Without this, New File /
   Rename / Move to Trash in projects that don't use file-system-synchronized
   groups produce files no target compiles. ✅

## Phase 1: a normal iOS day without opening Xcode

1. Test ergonomics: gutter diamonds to run one test or class, re-run failed,
   test plan selection. ✅
2. Edit Scheme (minimal): run arguments, environment variables, launch options. ✅
3. Debugger depth: LLDB console (`po`/`p`/`expr`), conditional, log-message,
   symbolic and Swift error/exception breakpoints. ✅
4. Physical devices via `devicectl`: install, launch, console, debug attach. ✅
5. SwiftPM management: Add Package, Resolve/Update/Reset, Package
   Dependencies in the navigator. ✅
6. Plist and entitlements editors (reuse the project-settings panel), basic
   Signing & Capabilities. ✅
7. Bookmarks navigator (Xcode 27 moved ⌘3 to Bookmarks). ✅

## Phase 2: Zed-quality editing

1. Editor-engine decision: spike moving the gpui-component editor into a
   first-party crate (Apache-2.0) instead of accumulating vendor patches.
   Zed's editor is GPL and off-limits. ✅
2. Break up `Workspace` into Pane/Item; split panes and Xcode's
   assistant/counterpart pane (⌃⌘T, ⌃⌥⌘J).
3. Git: gutter hunks, inline blame, diff view, stage/commit/branch.
4. sourcekit-lsp depth: inlay hints, semantic tokens, signature help, call
   hierarchy (⌃⇧⌘H), type hierarchy.
5. Multibuffers for project search and diagnostics.
6. Integrated terminal.
7. Vim mode.
8. File-based settings, keymap and themes; import Xcode `.xccolortheme` and
   `.idekeybindings`.

## Phase 3: AI

1. ACP client: host Claude Code / Codex / Gemini agents in a panel (Xcode 27's
   ⌘0 is Coding Assistant).
2. zcode as an MCP server: build, run, test, diagnostics, simulator
   screenshots (builds on the `debug.rs` control channel).
3. Inline edit predictions (later).

## Phase 4: Xcode-only surfaces, pragmatically

1. Profile (⌘I) via `xctrace` → Instruments.
2. Archive / export (`xcodebuild archive`, `-exportArchive`).
3. Asset catalog viewer (read-only first).
4. SwiftUI Previews: "open this preview in Xcode" bridge first.
5. Interface Builder: out of scope; "Open in Xcode".

## Foundations

1. Packaging: signed, notarized `.app`, updates, CLI install, `.xcodeproj`
   file association, login-item toggle for `--background`.
2. Scale benchmarks on a large open-source app (e.g. firefox-ios) with e2e
   gates for indexing, scan, search latency and memory.
3. Built-in Build Server Protocol server for sourcekit-lsp instead of the
   scraped compilation database.

# Xcode parity

Status of each item from the MVP list in [`xcode-inventory.md`](xcode-inventory.md),
which was extracted from Xcode 27's own command definitions.

✅ done · 🟡 partial · ⬜ not yet

## P0: edit, build, run on a simulator

| Item | Status | Notes |
|---|---|---|
| Open a folder / `.xcodeproj` / `.xcworkspace` / `Package.swift`, project navigator | ✅ | Context menu: Show File Inspector, New File, Add Files, Rename, Move to Trash, Show in Finder |
| Project navigator edits keep `project.pbxproj` consistent (file references, groups, build phases) | ✅ | Group-based projects; synchronized folders need no edits. Add Files… (⌥⌘A) with copy, groups or folder references, and targets. Moves are renames to a path (no drag and drop) |
| File inspector (⌥⌘1) with Target Membership | ✅ | Identity and Type plus Target Membership; read-only in synchronized folders. No Localization, On Demand Resource Tags or Text Settings sections |
| Source editor with tabs, highlighting, Save/Save All, undo, find/replace, ⌘/, ⌘[ ⌘], ⌥⌘[ ⌥⌘], ⌘L | ✅ | |
| sourcekit-lsp: completion, diagnostics, hover, ⌃⌘J, references, ⌃6 | ✅ | `.xcodeproj` indexing through a compilation database pulled from `xcodebuild` output (no xcode-build-server needed). Also semantic highlighting, signature help and inlay hints (Editor ▸ Inlay Hints), which Xcode doesn't have |
| Open Quickly (⇧⌘O), Quick Actions (⇧⌘A) | ✅ | Files and workspace symbols, `file:line` |
| Scheme and destination pickers | ✅ | `xcodebuild -list`, `simctl list`, `devicectl list devices` (polled while the window is active), Any iOS Device; grouped as in Xcode and filtered by `SUPPORTED_PLATFORMS`; Debug/Release |
| Build, Run, Stop, Clean (⌘B ⌘R ⌘. ⇧⌘K) | ✅ | Simulator boot, install and launch; Mac executables; Swift packages |
| Issue navigator, ⌘' / ⌘" | ✅ | Live and build issues, de-duplicated, shown inline; Show Issues in Editor puts them all, with their lines and messages, in a multibuffer |
| Console / debug area (⇧⌘Y, ⌘K) | ✅ | App stdout/stderr via `simctl launch --console-pty` |
| Find in Workspace (⇧⌘F), Find and Replace in Workspace (⌥⇧⌘F) | ✅ | Parallel and gitignore-aware; case, whole word and regex options. Results (and Find Selected Symbol's references) open as a multibuffer to edit in place; Replace All edits them there, one ⌘Z undoes it across files. No Replace for one result |

## P1: things people use every day

| Item | Status | Notes |
|---|---|---|
| Debugging with lldb-dap: breakpoints (⌘\\, ⌘Y), pause/continue (⌃⌘Y), step (F6/F7/F8), Debug navigator, Variables view, Breakpoints navigator | ✅ | Attaches to simulator and device apps and launches Mac executables; breakpoints are persisted and follow edits and undo (a breakpoint deleted with its line comes back with ⌘Z) |
| Breakpoint editing (Edit Breakpoint…, double-click): condition, ignore count, actions (Log Message with `@expr@`/`%B`/`%H`, Debugger Command, Sound), auto-continue, enable/disable | ✅ | Gutter markers with click, double-click and right-click as in Xcode. Actions run in zcode when the breakpoint stops |
| Symbolic (⌥⌘\\), Swift Error and Exception breakpoints (Breakpoint navigator + menu, Debug ▸ Breakpoints) | 🟡 | Symbol and module; Objective-C/C++ on throw or catch. No Swift error type filter, Test Failure, Runtime Issue or Constraint Error breakpoints, or watchpoints |
| LLDB console (`po`, `p`, `v`, `expr`, any lldb command) | ✅ | `(lldb)` prompt while paused, ↑/↓ history, Return repeats; Variables view Copy Value and Print Description; values on hover while paused |
| Test (⌘U), Tests navigator | ✅ | Unit tests run in the simulator by injecting the bundle into the host app; XCTest and Swift Testing results; failures shown as issues. UI tests need `xcodebuild test` |
| Test diamonds, run one test or class (gutter, Test navigator), Test Selected Items (⌃⌥⌘U), Test Again (⌃⌥⌘G), Clear Test Results (⌃⌥⌘K), re-run failed | ✅ | Tests found in source (Swift and Objective-C); selections reach xcodebuild (`-only-testing`), `swift test` (`--filter`) and the in-simulator runner (XCTest scope). Re-run failed has no Xcode menu equivalent (Xcode offers it in test reports). No Test Without Building, Test Repeatedly or Profile Test |
| Test plans | 🟡 | Choose a scheme's plan (Product ▸ Choose Test Plan…, Test navigator); runs honor it with every runner. No test plan editor (⌘>) |
| Navigation history ⌃⌘← ⌃⌘→, counterpart ⌃⌘↑, reveal ⇧⌘J | ✅ | History is per pane, as in Xcode |
| Bookmarks navigator (⌘3), Bookmark Line ⌃⌘' / Bookmark File ⌃⇧⌘' (and With Dialog), Jump to Next / Previous Bookmark ⌃; ⌃: (In Workspace ⌃⌥; ⌃⌥:) | ✅ | Gutter ribbons that follow edits; grouped by file with in-place descriptions, Mark as Complete, drag to reorder, Sort By, Delete All Completed Items, filter; Find navigator results and whole queries (Find Again). No free-form groups or cut/copy/paste of bookmarks. ⌘1…⌘9 follow Xcode 27 (no Symbol navigator: Document Items ⌃6) |
| Rename, Edit All in Scope (⌃⌘E), fix-its (⌃⌥⌘F) | ✅ | Rename shows the places it changed in a multibuffer, unsaved, undone in one ⌘Z; the index refreshes in the background after save |
| Re-indent (⌃I), Format File (⌃⇧I) | ✅ | Via sourcekit-lsp (swift-format) |
| Split panes / assistant | ✅ | Editor Pane on Right / Below (⌃⌘T ⌃⌥⌘T), Close Editor Pane (⌃⇧⌘W), Close Other Editor Panes, Close Other Tabs, Move Focus to Next / Previous Editor Pane (⌃\` ⌃⇧\`), Focus Editor Pane (⌃⇧⌘↩); one buffer in several panes (shared edits, undo and anchors, a cursor each); the assistant (⌃⌥⌘↩, ⌘↩) in Counterparts (.h/.m) or Manual, ⌥-click / ⌥-Return / ⌃⌥⌘J / ⌃⌥⌘↑ open in it; layout and sizes restored. No other assistant modes, no ⌘J pane chooser, no dragging tabs between panes |
| File ▸ Open Terminal at Location (and the navigator's context menu) | ✅ | Opens zcode's own terminal in the item's folder (Xcode opens Terminal.app); see the terminal below |
| Git: change gutter, blame, commit | ✅ | Change bars against `HEAD` as you type (staged ones hollow) with a popover: Discard Change, Stage / Unstage Change; Next / Previous Change (⌃\\ ⌃⇧\\); inline blame and Editor ▸ Authors (⌃⇧⌘A); Code Review as a tab, side by side or inline (⌥⇧⌘↩: not in Xcode 27's inventory); Integrate ▸ Commit… (⌥⌘C) with per-file and per-hunk staging and Amend, Push…, Pull… (⌥⌘X), Fetch Changes, Stage / Unstage / Discard All, Switch Branch… / New Branch…; Source Control navigator with staged and unstaged changes and branches. No stash, cherry-pick, tags, history or conflict editor |
| Swift packages: build, run, test | ✅ | |
| Swift package management: Package Dependencies in the navigator, Add Package Dependencies…, File ▸ Packages (Reset Package Caches, Resolve Package Versions, Update to Latest Package Versions), Update Package, Remove Package, Add Local… | 🟡 | Xcode projects, workspaces and Swift packages; versions from tags for every dependency rule; products to targets; read-only package sources; errors under Package Loading. No package collections search, registry packages, editing a package's rule, or package plugins |
| Build configuration picker | ✅ | The Run action's configuration, written to the scheme |
| Edit Scheme (⌘<), Run… (⌥⌘R), Test… (⌥⌘U), Next / Previous Scheme | ✅ | Run and Test actions: configuration, Debug executable, arguments and environment variables with checkboxes, working directory, persistent state, sanitizers, Main Thread Checker and memory diagnostics, written in place to the `.xcscheme` (packages: session). Applied to simulator, Mac, package and lldb-dap launches and to tests. No Build/Profile/Analyze/Archive actions, pre/post actions, scheme management, language/region or location options; test plans own their schemes' test options |
| Project editor (click the project in the navigator) | 🟡 | General fields and every build setting, per configuration, for the project and each target, written in place to `project.pbxproj`; Signing & Capabilities and Info tabs (below). No Build Phases, Build Rules or Resource Tags tabs, and no adding targets or configurations |
| Signing & Capabilities | 🟡 | Automatic signing, team (Xcode accounts, keychain identities), bundle ID, certificate, provisioning profile; + Capability for App Groups, iCloud, Push Notifications, Keychain Sharing, Background Modes, Associated Domains, Sign in with Apple, HealthKit, App Sandbox and Hardened Runtime with their options, written to the entitlements file (created and wired up), Info.plist or build settings. No per-configuration signing, other capabilities (edit their entitlements directly) or profile download/import |
| Property list editor (Info.plist, entitlements, privacy manifests, any plist), Open As ▸ Source Code / Property List | ✅ | Xcode's key names and value labels (Raw Keys and Values), key suggestions, Add Item, Value Type, move rows, XML and binary saved in their format with undo/redo through the text. No cut/copy/paste of rows |
| Info tab (Custom Target Properties) | ✅ | The target's Info.plist and `INFOPLIST_KEY_` settings; creates the Info.plist as Xcode does |
| Physical devices (devicectl) | 🟡 | Devices in the destination menu with their connection state; automatic signing (`-allowProvisioningUpdates`), install, launch with the scheme's arguments and environment, console, Stop, unlock wait; signing errors in the Issue navigator; lldb attaches through CoreDevice; `xcodebuild test` on the device. No Devices and Simulators window (⇧⌘2): pairing, Developer Mode and copying system symbols still need Xcode. Run, debugging and tests verified on an iPhone over the network; other hardware only against recorded `devicectl` output |

## P2

| Item | Status |
|---|---|
| Call/type hierarchy | ✅ Find Call Hierarchy (⌃⇧⌘H) with callers or callees, Find Ancestor / Descendent / Conforming Types, in the Find navigator; Editor ▸ Show Callers… |
| Folding, multi-cursor | ✅ (from the editor engine) |
| Vim mode (Editor ▸ Vim Mode) | ✅ Normal, insert, replace and visual (character, line, block) modes; operators with motions, text objects and counts; registers with the system clipboard; `.`, undo, search, marks, `:w` `:q` `:s` and ranges; the mode in the status bar; every editor tab and cursor. No macros or sentence objects (README, Known limitations) |
| Settings… (⌘,), font and color themes, custom key bindings | ✅ Zed's files instead of Xcode's panes: `~/.config/zcode/settings.json` (a project's `.zcode/settings.json` over it), `keymap.json` with contexts, JSON themes; Xcode's `.xccolortheme` themes and `.idekeybindings` key bindings import into them. Editor ▸ Invisibles; checkmarks on the Editor menu's toggles. No settings window, Relative Line Numbers or Code Coverage toggles |
| Coding Assistant (⌘0), File ▸ New ▸ Conversation (⌃⌘H) | ✅ An agent panel beside the inspector running Claude Code, Codex, Gemini CLI or any agent in `agent_servers` through the Agent Client Protocol (Xcode 27's Coding Assistant is a navigator and also runs ACP agents): streamed answers, tool calls with diffs, permission prompts, plan and modes; reads through zcode's buffers, writes as undoable transactions, Review against the file before the agent, Reject, Keep; @-mentions of files and symbols; Follow Agent. No conversation history, Coding Tools (⌥⌘0) or several conversations at once |
| Xcode's MCP server (`xcrun mcpbridge`): agents build, run and test through the IDE | ✅ `zcode mcp` (stdio, relayed to the running zcode): schemes and destinations listed and switched, build, run, stop, all or some tests with per-test results, the test list, the Issue navigator, the build log, the console, the current file and selection, Open File, LLDB in the paused frame, window and simulator screenshots; Xcode's tool names where it has the tool; progress and cancellation; handed to the agent panel's agents, which ask before building, running, testing or evaluating. No file, search, preview, snippet, build settings or Organizer tools (agents bring their own file tools) |
| Minimap | ⬜ (the editor's hooks exist: `docs/editor-engine.md`) |
| Reports navigator (log history) | ✅ |
| Profile (⌘I) with xctrace | ⬜ |
| Archive/export | ⬜ |
| SwiftUI Previews | ⬜ (needs private preview infrastructure) |

## Beyond Xcode (from Zed)

* A fuzzy command palette with every command and its shortcut (⇧⌘A / ⇧⌘P).
* Multibuffers: excerpts of many files in one editor tab, edited in place
  (find results, references, issues, renames), with file headers, each
  line's number in its file, change bars, breakpoints and diagnostics, ⌥↩ to
  open the file, ⇧↩ to show more lines, and one undo step across files.
* An integrated terminal: in the debug area beside the console and in
  editor tabs, several at once, the login shell in the project's folder,
  theme colors, scrollback, selection and ⌘C, ⌘-click on `file:line` and
  URLs, the mouse for programs that ask; fresh shells in the same folders
  after a relaunch (⌃⌘\`, View ▸ Activate Terminal; File ▸ New ▸ Terminal
  Tab). No IME composition, ⌥ as Meta or find in scrollback yet.
* Live memory monitoring for both the editor and the language server, a
  startup benchmark mode, and an idle-baseline memory view.
* Settings as files (Zed's): `settings.json` with comments, a project's
  own over it, `keymap.json` with key contexts (`Editor`, `vim_mode`,
  `Terminal`, `Navigator`…), JSON themes (also Zed's theme files), all
  applied as soon as they're saved, with mistakes shown and the last good
  settings kept; format on save, trailing whitespace, the terminal's shell.
* A scriptable UI control channel (`ZCODE_DEBUG_DIR`) for automated UI tests.

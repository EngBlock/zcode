# Xcode functionality inventory (for zcode MVP)

Source: `/Users/nathan/Downloads/Xcode-beta.app` (Xcode 27.0, build 27A5228h). `/Applications/Xcode.app` is also 27.0 (27A5252f) and has the same key navigator/inspector bindings. Both bundles were only read, never changed.

## Where the data lives

- **Menu bar and commands (the real source):** these are `Xcode.IDEKit.MenuDefinition` / `Xcode.IDEKit.CmdDefinition` extensions inside the plug-in manifests (`*.xcplugindata`, binary plists).
  - The main one is `Contents/Frameworks/IDEKit.framework/Resources/IDEKit.xcplugindata`.
  - The source editor adds its own Editor, Navigate and Find menus from `Contents/PlugIns/IDEPegasusSourceEditor.framework/Resources/IDEPegasusSourceEditor.xcplugindata`.
  - The debugger, source control and others add commands from their own plug-ins.
  - Across 207 manifests: 1146 CmdDefinitions, 248 MenuDefinitions and 254 CmdHandlers.
  - Shortcut fields are `keyEquivalent` plus `useControlKey`/`useOptionKey`/`useShiftKey`/`useCommandKey`. Cmd is on unless `useCommandKey=false`.
  - The root menu is `Xcode.IDEKit.MenuDefinition.Main`, with these menus: Xcode, File, Edit, View, Find, Navigate, Editor, Product, Debug, Integrate, Window, Help.
- **Text-editing keys:** `IDEKit.framework/Resources/IDEDefaultKeyBindings.idekeybindings` adds only a few Emacs/sub-word bindings. Examples: ⌃←/→ move by sub-word, ⌃⇧↑/↓ column select, ⌃. / ⌃> cycle completions, ⌃@ set mark, ⌃X⌃X swap with mark. `IDETextKeyBindingSet.plist` contains the base Cocoa text bindings.
- **Vi mode:** an `Xcode.KeyBindings.Vi` extension (`IDESourceEditor.IDEViEventConsumer`). It is switched on and off with Editor > Vim Mode. Its bindings are implemented in that class, not listed in a data file, so there is no table of them to check against; zcode's Vim mode (Phase 2.7) follows Vim's behavior and Zed's choices where Vim leaves them to the host.
- **Help:** there is no `.help` book in the bundle. Help menu items open online `doc://com.apple.documentation/...` links, so this inventory is based on the menus.

## Checking your guessed shortcuts

| Your guess | Bundle says | OK? |
|---|---|---|
| Open Quickly ⇧⌘O | ⇧⌘O | ✓ |
| Build ⌘B / Run ⌘R / Stop ⌘. / Test ⌘U | same | ✓ |
| Clean ⇧⌘K | ⇧⌘K "Clean Build Folder…" (⌥⇧⌘K cleans immediately, no prompt) | ✓ |
| Jump to Definition ⌃⌘J | ⌃⌘J (⌃⌥⌘J opens it in the assistant) | ✓ |
| Find in Project ⇧⌘F | ⇧⌘F, now called "Find in Workspace…" | ✓ |
| **Show Navigator ⌘0** | **✗ ⌘0 is now "Show Coding Assistant"** (AI chat navigator). "Show Navigator" has no shortcut in the manifest. | ✗ |
| Show Inspector (old ⌥⌘0) | **No shortcut. ⌥⌘0 is now "Show Coding Tools…"** | ✗ |
| Show Debug Area ⇧⌘Y | ⇧⌘Y (⇧⌘C activates the console) | ✓ |
| Comment ⌘/ | ⌘/ "Comment Selection" (⌥⌘/ adds a doc comment) | ✓ |
| Re-indent ⌃I | ⌃I | ✓ |
| Jump to line ⌘L | ⌘L "Jump to…" | ✓ |
| Back/Forward ⌃⌘← / ⌃⌘→ | same | ✓ |
| Next/Prev issue ⌘' / ⌘" | same | ✓ |
| Document Items ⌃6 | ⌃6 | ✓ |
| Find Call Hierarchy | ⌃⇧⌘H | (guess had no key) |
| Rename | **No default shortcut** (Editor > Refactor > Rename…) | n/a |
| Symbol navigator | **Not in Xcode 27.** ⌘3 now opens the **Bookmarks** navigator. | ✗ |

## Core menu commands with default shortcuts (only the ones relevant to the MVP)

### File
| Command | Key |
|---|---|
| New > File from Template… / Project… / Workspace… | ⌘N / ⇧⌘N / ⌃⌘N |
| New > Tab / Window / Editor Pane on Right / Below | ⌘T / ⇧⌘T / ⌃⌘T / ⌃⌥⌘T |
| Add Files… | ⌥⌘A |
| Open… / Open Quickly… | ⌘O / ⇧⌘O |
| Close Tab/Pane/Window / Close Other Tabs / Close Editor Pane / Close Workspace | ⌘W / ⌥⌘W / ⌃⇧⌘W / ⌃⌥⌘W |
| Close Other Editor Panes / Close Document | ⌃⌥⇧⌘W / ⌃⌘W |
| Save / Save All | ⌘S / ⌥⌘S |
| Show in Finder, Open Terminal at Location, Packages > Resolve/Update/Reset | — |

Xcode's Open Terminal at Location (`openTerminalAtLocation:`, also
`contextMenu_openTerminalAtLocation:` on the navigator) opens Terminal.app;
zcode's opens its own terminal there. Xcode has no integrated terminal:
zcode's terminal (Phase 2.6) follows Zed's terminal panel, in the debug
area beside the console, with View ▸ Activate Terminal on ⌃⌘\` (Zed's
⌃\` is Xcode's Move Focus to Next Editor Pane; IDEKit binds nothing to
⌃⌘\`) and File ▸ New ▸ Terminal Tab.

### Edit
| Command | Key |
|---|---|
| Undo / Redo / Cut / Copy / Paste / Select All / Duplicate | ⌘Z / ⇧⌘Z / ⌘X / ⌘C / ⌘V / ⌘A / ⌘D |
| Copy File and Line / Copy Symbol Name / Copy Qualified Symbol Name | ⌃⌥⌘C / ⌃⇧⌘C / ⌃⌥⇧⌘C |
| Bookmark Line / Bookmark File | ⌃⌘' / ⌃⇧⌘' |
| Bookmark Line With Dialog / Bookmark File With Dialog (⌥-alternates) | ⌃⌥⌘' / ⌃⌥⇧⌘' |

### View
| Command | Key |
|---|---|
| Navigators: Project, Source Control, Bookmarks, Find, Issues, Tests, Debug, Breakpoints, Reports | ⌘1 … ⌘9 |
| Show Coding Assistant | ⌘0 |
| Show Navigator / Show Inspector | (none) |
| Filter in Navigator | ⌥⌘J |
| Show Debug Area / Activate Console | ⇧⌘Y / ⇧⌘C |
| Editor > Related Items / Prev History / Next History / Top Level / Group Files / **Document Items** | ⌃1 / ⌃2 / ⌃3 / ⌃4 / ⌃5 / ⌃6 |
| Focus Editor Pane | ⌃⇧⌘↩ |
| Show Quick Actions… (command palette) | ⇧⌘A |
| Show Library | ⇧⌘L |
| Pin Editor Tab | ⇧⌘P |
| Show Toolbar / Enter Full Screen | ⌥⌘T / ⌃⌘F |

### Find
| Command | Key |
|---|---|
| Find… / Find and Replace… | ⌘F / ⌥⌘F |
| Find Next / Previous | ⌘G / ⇧⌘G |
| Use Selection for Find / Replace | ⌘E / ⇧⌘E |
| Select Next / Previous Occurrence (multi-cursor) | ⌥⌘E / ⌥⇧⌘E |
| Find in Workspace… / Find and Replace in Workspace… | ⇧⌘F / ⌥⇧⌘F |
| Find Next / Previous in Workspace | ⌃⌘G / ⌃⇧⌘G |
| Find Selected Symbol in Workspace | ⌃⇧⌘F |
| Find Call Hierarchy | ⌃⇧⌘H |
| Find Ancestor / Descendent / Conforming Types | — |

Xcode shows Find Call Hierarchy and the type commands as trees in the Find
navigator (`IDEFindNavigatorCommands`); the source editor also has Editor ▸
Show Callers… (a menu of callers, `jumpToCallers:`). Xcode has no inlay hints
or signature help popover.

Find and Replace in Workspace… shows the Find navigator with a Replace field
and Replace / Replace All buttons. Xcode has no multibuffer: zcode's Replace
All edits the results in one (Phase 2.5), and Show Find Results in Editor,
Show Issues in Editor, Open Excerpts (⌥↩) and Expand Excerpts (⇧↩, both in a
multibuffer only) are zcode's, after Zed's.

### Navigate (global menu plus the source editor's additions)
| Command | Key |
|---|---|
| Reveal in Project Navigator / Changes Navigator / Debug Navigator | ⇧⌘J / ⇧⌘M / ⇧⌘D |
| Jump to Definition (assistant: +⌥) | ⌃⌘J |
| Jump to… (line) | ⌘L |
| Jump to Selection | ⌥⌘L |
| Back / Forward in history | ⌃⌘← / ⌃⌘→ |
| Next / Previous Counterpart (.h/.m, generated interface) | ⌃⌘↑ / ⌃⌘↓ |
| Next / Previous Issue | ⌘' / ⌘" |
| Next / Previous Placeholder | ⌃/ / ⌃⇧/ |
| Next / Previous Change (diff hunk) | ⌃\ / ⌃⇧\ |
| Next / Previous Bookmark | ⌃; / ⌃⇧; |
| Next / Previous Bookmark In Workspace | ⌃⌥; / ⌃⌥⇧; |
| Previous / Next Tab | ⇧⌘[ / ⇧⌘] |
| Move Focus to Next Area / Next Editor Pane / Editor Pane… | ⌥⌘\` / ⌃\` / ⌘J |
| Move Focus to Previous Editor Pane | ⌃⇧\` |
| Jump to Next Counterpart in the assistant (⌥) | ⌃⌥⌘↑ |
| Jump to Instruction Pointer (debugging) | ⌃⌘P |

### Editor (source editor context)
| Command | Key |
|---|---|
| Show Editor Only / Canvas (Preview) / Assistant | ⌘↩ / ⌥⌘↩ / ⌃⌥⌘↩ |
| Show Completions / Show Predictive Completion (`predict:`, right after it in the Editor menu; Xcode's model suggests while you type when Settings ▸ Text Editing ▸ Predictive code completion is on, `automaticallySuggestPredictiveCodeCompletion` in `SourceEditor`) | ⌃Space / ⌃⌥Space |
| Edit All in Scope | ⌃⌘E |
| Refactor > Rename…, Extract to Function/Method/Variable, Add Missing Protocol Requirements, Add Missing Switch Cases, Generate Memberwise Initializer | — |
| Show Callers…, Expand Macro | — |
| Fix All Issues / Show Issue / Show All Issues | ⌃⌥⌘F / ⌃⌥⌘L / ⌃⌘L |
| Structure > Re-Indent / Format File / Format Selection | ⌃I / ⌃⇧I / ⌃⌥⇧I |
| Shift Left / Shift Right | ⌘[ / ⌘] |
| Move Line Up / Down | ⌥⌘[ / ⌥⌘] |
| Comment Selection / Add Documentation | ⌘/ / ⌥⌘/ |
| Fold / Unfold / Fold Methods / Fold Comments | ⌥⌘← / ⌥⌘→ / ⌥⇧⌘← / ⌃⇧⌘← |
| Select Column Up / Down | ⌃⇧↑ / ⌃⇧↓ |
| Font Size Increase / Decrease / Reset | ⌘+ / ⌘- / ⌃⌘0 |
| Minimap / Authors (blame) / Wrap Lines | ⌃⇧⌘M / ⌃⇧⌘A / ⌃⇧⌘L |
| Toggles: Vim Mode, Relative Line Numbers, Invisibles, Code Coverage, Inline/Side-by-Side Comparison | — |
| Code Review (not listed in 27's definitions; ⌥⌘↩ is Canvas now, ⌥⇧⌘↩ in Xcodes that have it) | — |

### Product
| Command | Key |
|---|---|
| Build / Run / Test / Profile / Analyze | ⌘B / ⌘R / ⌘U / ⌘I / ⇧⌘B |
| Run… / Test… / Profile… (options dialog) | ⌥⌘R / ⌥⌘U / ⌥⌘I |
| Stop | ⌘. |
| Build For > Running / Testing / Profiling | ⇧⌘R / ⇧⌘U / ⇧⌘I |
| Run / Test / Profile Without Building | ⌃⌘R / ⌃⌘U / ⌃⌘I |
| Test Selected Items / Test Again | ⌃⌥⌘U / ⌃⌥⌘G |
| Compile File / Analyze File | ⌃⌘B / ⌃⇧⌘B |
| Clean Build Folder… / Clean Immediately / Clear Test Results | ⇧⌘K / ⌥⇧⌘K / ⌃⌥⌘K |
| Build Documentation | ⌃⇧⌘D |
| Archive, Delete Derived Data…, Show Build Folder in Finder | — |
| Scheme > Choose / Next / Previous / Edit Scheme… | ⌃0 / ⌃⌘] / ⌃⌘[ / ⌘< |
| Destination > Choose / Next / Previous / Manage Devices… | ⌃⇧0 / ⌃⌥⌘] / ⌃⌥⌘[ / ⇧⌘2 |
| Test Plan > Edit Test Plan | ⌘> |

### Debug
| Command | Key |
|---|---|
| Pause / Continue | ⌃⌘Y |
| Step Over / Into / Out | F6 / F7 / F8 |
| Step Over / Into Instruction | ⌃F6 / ⌃F7 |
| Continue to Current Line | ⌃⌘C |
| Activate (toggle all) Breakpoints | ⌘Y |
| Create Breakpoint at Current Line / Symbolic Breakpoint… | ⌘\ / ⌥⌘\ |
| Exception / Swift Error / Test Failure breakpoints, Delete All | — |
| Console: Clear / Reload / Jump to Source | ⌘K / ⌥⌘K / ⌥⇧⌘J |
| View Memory / Interrupt Debugger | ⌃⌥⇧⌘M / ⌃C |
| Attach to Process (by PID or name), Detach, Debug Executable…, Simulate Location, Capture View Hierarchy | — |

### Integrate (source control) and Window
| Command | Key |
|---|---|
| Commit… / Pull… | ⌥⌘C / ⌥⌘X |
| Push, Fetch, Stage, Unstage, Stash, Cherry-Pick, Discard, Clone, Create Pull Request | — |
| Window: Previous / Next Window Tab | ⌃⇧⇥ / ⌃⇥ |
| Developer Documentation / Welcome / Organizer | ⇧⌘0 / ⇧⌘1 / ⌥⇧⌘O |
| Help: Quick Help for Selected Item / Search Docs for Selection | ⌃⌘? / ⌃⌥⌘/ |
| Settings… | ⌘, |

Xcode's Settings… (`showPreferencesPanel:`) opens a window of panes; its
font and color themes are `.xccolortheme` property lists (the user's in
`~/Library/Developer/Xcode/UserData/FontAndColorThemes`, Xcode's own in
`SharedFrameworks/DVTUserInterfaceKit.framework/…/FontAndColorThemes`) and
its custom key bindings are `.idekeybindings` (`UserData/KeyBindings`: a
Menu Key Bindings list of commands by action selector and command ID with
their `Keyboard Shortcut`, in Xcode's notation, `@` ⌘ `^` ⌃ `~` ⌥ `$` ⇧,
and Text Key Bindings, Cocoa text selectors by keystroke; IDEKit's own
default is an old-style property list). zcode's Settings… (⌘, as in Xcode)
opens `settings.json`, as Zed's does (Phase 2.8); zcode ▸ Key Bindings…,
Select Theme… and the two Xcode importers sit beside it. Editor ▸
Invisibles is Xcode's `toggleInvisibleCharactersShown:`.

## Workspace areas and concepts

**Window layout:** Toolbar (Run/Stop, scheme and destination picker, activity/status view) · **Navigator area** (left) · **Editor area** (tabs, split panes, jump bar, assistant/canvas, minimap) · **Debug area** (bottom: variables view plus LLDB/console) · **Inspector area** (right).

**Navigators** (`Xcode.IDEKit.NavigatorGroup`, listed by `preferredPosition`):
1. **Project** ⌘1: workspace/project file tree (`IDEWorkspaceNavigator`)
2. **Source Control** ⌘2: Changes and Repositories (branches, tags, remotes)
3. **Bookmarks** ⌘3: replaces the old Symbol navigator. The Symbol navigator does not exist in 27. Its context menu (IDETrayKit): Show in Finder, Reveal in Project Navigator, Edit Description, Mark as Complete, Open in Tabs, Cut/Copy/Paste/Delete/Duplicate Bookmark, Sort By Name / Type / Completion Status, New Group, New Group from Selection; a filter with "Show only incomplete items" and Delete All Completed Items. Find navigator queries become bookmark groups of their results.
4. **Find** ⌘4: Find navigator, with a "Search" navigator as an alternate
5. **Issues** ⌘5: build and live diagnostics (`IDENoticeNavigator`)
6. **Tests** ⌘6: test explorer
7. **Debug** ⌘7: threads and stack frames, plus CPU/memory gauges
8. **Breakpoints** ⌘8
9. **Reports** ⌘9: Local build/test/run logs, plus Xcode Cloud
10. **Coding Assistant** ⌘0: AI chat (new; it took ⌘0). Its command is
    View ▸ Navigators ▸ Show Coding Assistant (`toggleCodingAssistantNavigator:`);
    File ▸ New ▸ Conversation is ⌃⌘H (`newCodingAssistant:`, ⌥ for a new
    tab, ⌥⇧ for an editor pane), Jump to Next Active Conversation ⌃⌘K and
    Show Coding Tools… ⌥⌘0. Xcode's `IDEIntelligenceAgents` plug-in
    registers Claude Code and Codex, and `IDEIntelligenceACP` custom
    Agent Client Protocol agents. zcode's agent panel (Phase 3.1) takes ⌘0
    and ⌃⌘H and sits on the inspector's side instead of in the navigator
    area, as Zed's agent panel does, so the project navigator stays in view
    while an agent works; Show/Hide Navigator, which zcode had on ⌘0, has no
    key, as in Xcode 27.
    Xcode 27 is also an MCP server for agents: `xcrun mcpbridge` relays
    stdio to a running Xcode's tool service (`MCP_XCODE_PID` picks one; it
    exits when no Xcode runs). Its tools, as `IDEIntelligenceChat` lists
    them: `BuildProject`, `RunProject`, `StopProject`, `RunAllTests`,
    `RunSomeTests`, `GetTestList`, `GetBuildLog`, `GetConsoleOutput`,
    `XcodeListNavigatorIssues`, `XcodeRefreshCodeIssuesInFile`,
    `XcodeListSchemes`, `XcodeSwitchScheme`, `XcodeListRunDestinations`,
    `XcodeSwitchRunDestination`, `XcodeGetCurrentFile`, `XcodeListWindows`,
    `XcodeListWorkspaces`, `XcodeOpenWorkspace`, `XcodeCloseWorkspace`,
    file tools (`XcodeRead`, `XcodeWrite`, `XcodeUpdate`, `XcodeInsertFile`,
    `XcodeGlob`, `XcodeGrep`, `XcodeLS`, `XcodeMakeDir`, `XcodeMV`,
    `XcodeRM`), `RenderPreview`, `RunCodeSnippet`, `GetFileCompilerFlags`,
    `GetTargetBuildSettings` and Organizer crash and performance logs.
    `zcode mcp` (Phase 3.2) works the same way and uses these names for the
    tools it shares; it adds `OpenFile`, `EvaluateInDebugger` and
    `TakeScreenshot`.

**Inspectors (general, not Interface Builder):** File, History (source control log), Quick Help, Source Control. Interface Builder and other editors add Identity, Attributes, Size, Connections, Bindings and so on. The view and memory debuggers add Object, Size and Memory.

**Editor-area concepts:**
- Jump bar, with the ⌃1–⌃6 menus. ⌃6 is the document symbol outline.
- Tabs, which can be pinned.
- Split editor panes, plus the Assistant (counterpart, callers and similar).
- Canvas (SwiftUI Previews).
- Minimap, authors/blame, inline diff.
- Code folding, placeholders, multi-cursor.
- Quick Actions palette (⇧⌘A).

**Build and run concepts:**
- **Scheme selector:** a scheme maps the Build, Run, Test, Profile, Analyze and Archive actions to targets. Each action has a build configuration (Debug/Release), plus arguments, environment variables and diagnostics options.
- **Run destination picker:** My Mac, connected devices, iOS/watchOS/tvOS/visionOS simulators, and "Any … Device".
- **Test plans:** configurations of tests, picked per scheme.
- **Build configurations:** Debug and Release by default, plus any custom ones.
- **Derived Data** and the build folder.
- **Swift Package dependencies:** resolve, update and reset.

## Recommended MVP for zcode (uses xcodebuild / simctl / sourcekit-lsp / lldb-dap)

### P0: required for "edit, build, run on a simulator"
1. **Open a folder, `.xcodeproj`, `.xcworkspace` or `Package.swift`.** Project navigator file tree, file create/rename/delete. Only edit the files on disk; Xcode's pbxproj group view is P1.
2. **Source editor with tabs:** tree-sitter Swift/ObjC/C highlighting, Save / Save All, undo, find/replace in file (⌘F, ⌘G, ⌘E), comment (⌘/), shift (⌘[ / ⌘]), move line (⌥⌘[ / ⌥⌘]), go to line (⌘L).
3. **sourcekit-lsp:** completion (⌃Space), diagnostics shown inline, hover/Quick Help, Jump to Definition (⌃⌘J), Find References, Document Items outline (⌃6). Use `xcode-build-server` / BSP or a compile_commands-style index so `.xcodeproj` projects get indexed.
4. **Open Quickly (⇧⌘O)** for files and workspace symbols. Add a basic command palette (⇧⌘A).
5. **Scheme and destination pickers:**
   - Schemes come from `xcodebuild -list -json`.
   - Destinations come from `xcrun simctl list -j devices` and `xcrun devicectl list devices`, plus My Mac.
   - Configuration defaults to Debug.
6. **Build (⌘B), Run (⌘R), Stop (⌘.), Clean (⇧⌘K):**
   - Build with `xcodebuild build -scheme … -destination …`, using `-resultBundlePath` and `-showBuildSettings` to find the product.
   - For a simulator run: `simctl boot`, then `simctl install`, then `simctl launch --console-pty`.
   - For macOS: launch the .app directly.
   - Stream the output into a build log panel.
7. **Issue navigator (⌘5)** filled from build errors (xcresult or parsed log), with next/previous issue (⌘' / ⌘").
8. **Console / debug area (⇧⌘Y)** showing app stdout/stderr, with Clear (⌘K).
9. **Find in workspace (⇧⌘F)** using ripgrep, with a results navigator (⌘4).

### P1: the parts of "an Xcode alternative" people use every day
1. **Debugging with lldb-dap:**
   - Breakpoints in the gutter (⌘\, ⌘Y to toggle all).
   - Pause/Continue (⌃⌘Y), Step Over/Into/Out (F6/F7/F8).
   - Threads and stack frames in a Debug navigator (⌘7), variables view, LLDB REPL in the console.
   - A Breakpoints navigator (⌘8).
2. **Test (⌘U):** `xcodebuild test` with xcresult parsing, a Tests navigator (⌘6) with run-single-test, Test Again (⌃⌥⌘G), and failures shown inline.
3. **Navigation history:** ⌃⌘← / ⌃⌘→, counterpart switching (⌃⌘↑), reveal in navigator (⇧⌘J).
4. **Refactors through LSP code actions:** Rename (add a shortcut; Xcode ships none), Edit All in Scope (⌃⌘E), quick-fix / Fix All Issues (⌃⌥⌘F).
5. **Formatting:** Re-indent (⌃I), Format File (⌃⇧I) via `swift-format`.
6. **Split panes and assistant:** ⌃⌘T, Focus Editor Pane.
7. **Git:** gutter change markers, next/previous change (⌃\), blame/Authors (⌃⇧⌘A), plus a simple Changes navigator with stage/commit (⌥⌘C).
8. **Swift Package support:** resolve/update packages, `swift build` / `swift test` for package-only workspaces.
9. **Build settings visibility:** pick the configuration, plus Run options dialog (⌥⌘R) for arguments and environment variables. Edit Scheme (⌘<) as a simple form.
10. **Real-device run:** `xcrun devicectl device install app` then `process launch`. Signing is handled by the existing project settings with `-allowProvisioningUpdates`.

### P2: later, or not at all
- Call hierarchy (⌃⇧⌘H) and type hierarchy (LSP supports both). Done in zcode (Phase 2.4).
- Code folding, minimap, Vim mode, multi-cursor extras.
- Quick Help / documentation viewer, plus a DocC build (⌃⇧⌘D).
- Reports navigator (build log history), timing summary.
- Profile (⌘I) by handing off to Instruments with `xctrace`.
- Simulate Location, view-hierarchy capture, memory graph.
- Archive and export (`xcodebuild archive` / `-exportArchive`), Organizer.
- SwiftUI Previews canvas. This is hard because it needs private preview infrastructure, so consider a "run in simulator with hot reload" alternative.
- Bookmarks navigator, snippets library, coding-assistant chat.

**Out of scope for zcode:** Interface Builder/storyboards, Core Data/SpriteKit/SceneKit/RealityKit editors, asset-catalog editing beyond basics, Xcode Cloud, StoreKit/GameKit test tooling, and the GPU debugger.

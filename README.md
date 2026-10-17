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
| **Projects** | Opens `.xcodeproj`, `.xcworkspace`, Swift packages and plain folders. Includes a welcome window with recent projects and per-project session restore (panes and tabs, scheme, destination, breakpoints, layout). | ⌘O |
| | Project settings: click the project in the navigator to edit the project and its targets (General fields such as bundle ID, version, deployment targets and team, plus every build setting per configuration). Edits are written straight to `project.pbxproj` and keep its formatting. The panel can also open `project.pbxproj` as text or open the project in Xcode. | — |
| | Signing & Capabilities for each target: Automatically manage signing, Team (from Xcode's signed-in accounts and the keychain's signing identities, or typed), Bundle Identifier, Signing Certificate (Development, Distribution, Sign to Run Locally, or a keychain identity) and Provisioning Profile (installed profiles that match the team and bundle ID). + Capability adds App Groups, iCloud, Push Notifications, Keychain Sharing, Background Modes, Associated Domains, Sign in with Apple, HealthKit, and on the Mac App Sandbox and Hardened Runtime, each with its options: entitlements go into the target's `.entitlements` file (created, set as `CODE_SIGN_ENTITLEMENTS` and added to the project the first time), Background Modes into its Info.plist, App Sandbox and Hardened Runtime into build settings, as Xcode 15+ keeps them. Builds register them with Apple through `-allowProvisioningUpdates`; zcode never calls the developer portal. Signing errors open this tab. | — |
| | The target's Info tab: Custom Target Properties, its Info.plist keys and `INFOPLIST_KEY_` build settings together in the property list editor. The first key added to a target with a generated Info.plist creates the file next to its sources and sets `INFOPLIST_FILE` (in a synchronized folder, with the membership exception that keeps it out of the target's resources). | — |
| **Property lists** | `Info.plist`, `.entitlements`, `.xcprivacy`, `.stringsdict` and any binary or XML `.plist` open in a Key / Type / Value outline: disclosure for dictionaries and arrays, Xcode's names for keys and values ("Bundle display name", "Privacy - Camera Usage Description", "Portrait (bottom home button)") from Xcode's own definitions, Editor ▸ Raw Keys and Values for the raw ones, + and − on each row, Editor ▸ Add Item and Value Type (Array, Dictionary, Boolean, Data, Date, Number, String), key suggestions as you type, menus for Booleans and allowed values, and rows moved with ⌥⌘[ ⌥⌘] or by dragging. Every change edits the file's text in place, so undo, redo, the dirty dot and ⌘S work as in source code, an unmodified file saves byte for byte, and new values are written as Xcode writes them (tabs, the `<!DOCTYPE>` header). Binary plists are saved as binary, keys in their order. Open As ▸ Source Code / Property List (Editor menu, or the navigator's context menu) switches views. | ⌥⌘[ ⌥⌘], ⌘Z |
| **Swift packages** | Package Dependencies at the bottom of the project navigator: every resolved package with its version (or branch, or commit), expandable to browse its sources, which open read-only (with a lock on the tab), as do package sources Jump to Definition reaches. | — |
| | File ▸ Add Package Dependencies…: enter a repository URL or a GitHub `owner/repo`; its versions come from its tags (`git ls-remote`) for the dependency rule (Up to Next Major Version, Up to Next Minor Version, Range of Versions, Exact Version, Branch, Commit). Add Package fetches that version to read its products; then pick the target each product is added to. Add Local… adds a package folder. In an `.xcworkspace` you choose which project gets the package. | — |
| | File ▸ Packages ▸ Reset Package Caches, Resolve Package Versions, Update to Latest Package Versions; Update Package and Remove Package on a package's context menu. Xcode projects are edited in `project.pbxproj` as Xcode writes it (package references, product dependencies, the targets' Frameworks phase) and resolved with `xcodebuild -resolvePackageDependencies` into the DerivedData folder builds use; Swift packages use `swift package add-dependency`, `add-target-dependency`, `resolve`, `update` and `reset`. Progress shows in the activity view, errors under Package Loading in the Issue navigator, and the index refreshes afterwards so new modules can be imported. | — |
| **Editor panes** | File ▸ New ▸ Editor Pane on Right / Below splits the focused pane, showing its file at the same place; panes split further side by side or stacked and are resized by dragging. Each pane has its own tabs, jump bar and Go Back / Go Forward history. A file shown in several panes is one document: an edit, undo or redo in one pane shows in the others at once, each pane keeps its own cursor and scroll position, and breakpoints, bookmarks, test diamonds, issues, the execution mark and the property list outline appear in every pane. Close Editor Pane, Close Other Editor Panes, Close Other Tabs; closing a pane's last tab closes the pane. Move Focus to Next / Previous Editor Pane, or click; Focus Editor Pane shows the focused pane alone. The layout, tabs, sizes and focus are restored with the session. | ⌃⌘T ⌃⌥⌘T, ⌃⇧⌘W ⌃⌥⇧⌘W ⌥⌘W, ⌃\` ⌃⇧\`, ⌃⇧⌘↩ |
| | The assistant (Editor ▸ Assistant) opens beside the focused editor in Counterparts mode, following it with the `.h` / `.m` (`.c`, `.cpp`, `.mm`) counterpart of its file, or in Manual mode with what was opened in it. ⌥-click in the project navigator, ⌥-Return (or ⌥-click) in Open Quickly, Jump to Definition and Jump to Next Counterpart with ⌥ open in the assistant (or the next pane, or a new assistant). Show Editor Only closes it. | ⌃⌥⌘↩, ⌘↩, ⌃⌥⌘J, ⌃⌥⌘↑ |
| **Terminal** (from Zed) | Terminals in the debug area's Terminal tab, beside the console (several at once, each with its tab and + for another), and in editor panes (File ▸ New ▸ Terminal Tab): the login shell on a pseudo-terminal, started in the project's folder. File ▸ Open Terminal at Location (also on the navigator's context menu) opens one in the selected item's folder. The editor's font and size, the 16 ANSI colors from the theme (so light and dark both read), 256 and 24-bit color, bold, italic, underlines; 10,000 lines of scrollback (the wheel, ⇧PageUp/PageDown); drag to select (double-click a word, triple-click a line, ⇧-click to extend), ⌘C, ⌘V (bracketed when the program asks), ⌘A; ⌘K clears it. ⌘-click a `file:line:column` (compiler errors, `grep -n`, test failures) to open the editor there, a URL to open the browser, or a link a program marks (OSC 8). Programs that ask get the mouse (`vim`, `htop`), and full-screen ones get the wheel as arrows. Plain and ⌃ keys go to the shell (⌃C, ⌃\\, ⌃6 included), ⌘ keys stay zcode's (⌘W closes the terminal). A tab is named after what's running in it. Closing a terminal, its tab or the window ends its processes; the session reopens terminals as fresh shells in the folder each was in. | ⌃⌘\`, ⌘K, ⌘W |
| **Editing** | Rope-based editor with tree-sitter highlighting (Swift, C/ObjC, C++, JSON, YAML, Markdown…), multiple cursors, folding, indent guides and find/replace. | ⌘F, ⌥⌘F |
| **Multibuffers** (from Zed) | One editor showing excerpts of many files, edited in place: Find ▸ Show Find Results in Editor (or the Find navigator's button) for Find in Workspace's results or Find Selected Symbol's references, Navigate ▸ Show Issues in Editor (or the Issue navigator's button) for every issue with its lines around it and its message under it, and a rename's changes. Each file has a header with its name and folder; the gutter shows each line's number in its file, its change bars and breakpoints (click one to toggle it), and the excerpts show the file's diagnostics. Typing in an excerpt edits the file itself (unsaved, as in its tab, which shows the change at once, and the other way around); ⌘S saves the files the tab changed, and one ⌘Z undoes one edit across every file it touched, in step with each file's own undo. Click a header or a line number, or ⌥↩, to open the file there; ⇧↩ or a header's ↕ shows three more lines around an excerpt. Find ▸ Find and Replace in Workspace adds a Replace field whose Replace All edits the results this way. A search over thousands of files opens a page of files at a time as the tab scrolls. | ⌥⇧⌘F, ⌥↩, ⇧↩ |
| | Comment selection, duplicate, move line, select next occurrence, soft wrap and font size. | ⌘/, ⌘D, ⌥⌘[ ⌥⌘], ⌥⌘E, ⌃⇧⌘L, ⌘+ ⌘- ⌃⌘0 |
| | Xcode-style `<#placeholder#>` snippets from completions, with Tab or ⌃/ jumping to the next placeholder. | Tab, ⌃/, ⌃⇧/ |
| **Settings** (from Zed) | zcode ▸ Settings… opens `~/.config/zcode/settings.json`: JSON with comments in Zed's format, applied the moment it's saved, in every window. `theme` (a name, or one for light and one for dark appearance), `buffer_font_family`, `buffer_font_size`, `tab_size`, `hard_tabs`, `soft_wrap`, `show_whitespaces`, `gutter.line_numbers`, `indent_guides`, `format_on_save` (through sourcekit-lsp, on ⌘S, Save All and `:w`), `remove_trailing_whitespace_on_save`, `ensure_final_newline_on_save`, `inlay_hints`, `vim_mode`, `git.inline_blame`, `git.show_authors`, `git.side_by_side_comparison` and `terminal.shell`; zcode ▸ Open Default Settings lists them with their defaults (read-only). A project's `.zcode/settings.json` (zcode ▸ Open Project Settings File) wins over yours for its window, except the theme, font family and Vim mode. The Editor menu's toggles (Authors, Inline Blame, Inlay Hints, Vim Mode, Wrap Lines, Invisibles, Side by Side / Inline Comparison) and ⌘+ ⌘- ⌃⌘0 write the file, keeping its comments, and show checkmarks. A mistake (a missing comma, `"tab_size": "four"`, an unknown setting) is shown with its line and column, and the settings it gave last stay until it's fixed. The toggles zcode kept in its session before move into the file once. | ⌘, |
| | zcode ▸ Key Bindings… opens `keymap.json`: blocks of bindings with an optional `context` (`Editor`, `Editor && vim_mode == normal`, `multibuffer`, `Terminal`, `Navigator && navigator == find`, `PlistEditor`, `Picker`, `Workspace`, …), over zcode's; an action by name (`zcode::Build` or `Build`, `[name, arguments]`), `null` to free a key, `["zed::Unbind", action]` to take a key off one action; multi-key sequences (`ctrl-x ctrl-s`). Bindings for a Vim mode come before Vim's own keys. Bindings that can't be made are reported; the rest apply. zcode ▸ Open Default Key Bindings lists zcode's. | — |
| | Themes are JSON files in `~/.config/zcode/themes`: gpui-component's format (the window's colors, the editor's and its syntax colors, a monospace font) or Zed's theme files. zcode ▸ Select Theme… lists them with Default Light and Default Dark. zcode ▸ Import Xcode Theme… converts one of Xcode's `.xccolortheme` themes (yours from `~/Library/Developer/Xcode/UserData/FontAndColorThemes`, or one Xcode ships) into a theme file and shows it: the editor's background, text, current line, selection, insertion point, invisibles, syntax colors with their bold and italic, and the font when it's installed. zcode ▸ Import Xcode Key Bindings… turns a set from `~/Library/Developer/Xcode/UserData/KeyBindings` into blocks of `keymap.json`: each command zcode has gets its shortcut, and loses zcode's default ones as in Xcode, and text editing bindings go to the editor; importing again replaces them. Xcode's files are only read. | — |
| **Vim mode** | Editor ▸ Vim Mode (off by default; `vim_mode` in the settings, for every window) in every editor tab: files in every pane, both sides of Code Review and multibuffers. Normal, insert, replace, visual, visual line and visual block modes, with the mode in the status bar (`-- INSERT --`, then any keys of a command not finished yet). Motions (`h j k l w W b B e E ge 0 ^ $ g_ \| gg G { } f F t T ; , % H M L + - _`, marks), counts, and the operators `d c y > < = g~ gu gU` over motions and text objects (`iw aw iW aW ip ap i" a" i' a' i( a( i[ a[ i{ a{ i< a< it at`, and the backtick quotes); `x X s S C D Y p P gp gP J gJ r R ~ o O i a I A gi gI ⌃A ⌃X`; registers (`"a`–`"z`, `"A`–`"Z` to append, `"0`–`"9`, `"-`, `"_`, `"+` `"*`, `".` `"/` `":`), the unnamed one being the system clipboard (as in Zed); `.` repeats the last change with what its insert typed (completions and closed brackets included); `u` and `⌃R`, an insert undoing in one step with the change that started it; `/ ? n N * #` with Vim's patterns and the matches highlighted; `m` with `'` and the backtick to go to marks; scrolling (`⌃D ⌃U ⌃F ⌃B ⌃E ⌃Y zz zt zb`) and folds (`zo zc za zR zM`); visual block `I`, `A` and `c` typing on every row at once. `:` commands: `:w`, `:wa`, `:q`, `:q!`, `:wq`, `:x`, `:qa`, `ZZ`, `ZQ` (`:q` closes the tab, as in Zed, and refuses unsaved changes without `!`; in a multibuffer `:w` saves the files the tab changed), `:s` with ranges (`%`, `.`, `$`, numbers, marks, `'<,'>`, `+N`) and the `g`, `i`, `I`, `n` flags, `:<line>`, `:d`, `:y`, `:j`, `:>`, `:<`, `:noh`, `:undo`, `:redo`, with ↑/↓ history. Every cursor of a multi-cursor editor moves and operates; a selection made with the mouse, ⌘D or a placeholder is visual mode. Escape closes completion, signature help and Find before it leaves insert mode. `gd` jumps to the definition, `gt` / `gT` switch tabs, `⌃O` / Tab go back and forward, `=` re-indents through sourcekit-lsp. zcode's own ⌃ shortcuts (⌃6, ⌃I, ⌃/, ⌃\`…) and every ⌘ key keep working; terminals, property list outlines and text fields aren't affected. | Editor ▸ Vim Mode |
| **Language intelligence** (sourcekit-lsp) | Completion, live diagnostics, Quick Help on hover, Jump to Definition (also ⌘-click), Find References and Rename across files. A rename opens a tab of the places it changed (a multibuffer, below), left unsaved: ⌘S saves every file, one ⌘Z undoes it everywhere. | ⌃Space, ⌃⌘J, ⌃⇧⌘F, F2 |
| | Edit All in Scope, Format File / Re-Indent, code actions and fix-its, Document Items outline, and workspace symbols in Open Quickly. | ⌃⌘E, ⌃⇧I / ⌃I, ⌃⌥⌘F, ⌃6 |
| | Semantic highlighting over tree-sitter's: what the compiler says each name is, so a type reads as a type at a call (`Square(side: 2)`), and properties, parameters and enum cases get their colors. Signature help while typing arguments: the call's signature above the cursor with the current argument in bold, from `(` until the call is closed (or Escape). Editor ▸ Inlay Hints (off by default, as in Zed): the type inferred for each binding without one (`let areas: [Double]`), and in C and Objective-C the parameter names before arguments (`add(left: 1, right: 2)`), drawn in the text without being part of it. | — |
| | Find ▸ Find Call Hierarchy: the function at the cursor (in whichever pane is focused) as a tree in the Find navigator, with its callers (as in Xcode) or the calls it makes, each level expanded on demand; a caller opens at its call, the rest at their declarations, and a row's menu starts a new hierarchy from it. Find Ancestor Types, Find Descendent Types and Find Conforming Types show a type's supertypes or subtypes the same way. Editor ▸ Show Callers… lists the callers in a menu at the cursor. Features the language server doesn't offer are left out, and their commands say so. | ⌃⇧⌘H |
| **Xcode project indexing** | zcode pulls a compilation database out of `xcodebuild` output, the job xcode-build-server does, so cross-file features work for `.xcodeproj` targets. When a project has never been built, a low-priority background "index build" produces it. Saving source files refreshes the index, so rename and references stay current. | — |
| **Navigation** | Open Quickly (fuzzy file and symbol search, `file:line`), Quick Actions command palette, back/forward history, jump to line, counterpart (.h ↔ .m), reveal in navigator and tab switching. | ⇧⌘O, ⇧⌘A, ⌃⌘← ⌃⌘→, ⌘L, ⌃⌘↑, ⇧⌘J, ⇧⌘[ ⇧⌘] |
| | Group-based projects (no file-system-synchronized folders) stay consistent as Xcode keeps them: New File, Rename (a name with `/` moves the item), Move to Trash and Add Files… add, update and remove file references, groups and build files in `project.pbxproj` (Sources for code, Resources for resources), with Xcode-style IDs and the rest of the file left byte for byte. New files join the targets that build their neighbors. | ⌥⌘A |
| | File inspector: name, type, location, full path and Target Membership checkboxes for the selected file. | ⌥⌘1 |
| **Navigators** | Xcode 27's set and order: Project (file tree with New File / Add Files / Rename / Move to Trash / Bookmark File / Show File Inspector / Show in Finder), Source Control (the branch and how far it is from its upstream, staged and unstaged changes with Show Changes / Stage / Unstage / Discard Changes, branches), Bookmarks, Find (find in workspace with case, word and regex options), Issues (live + build), Tests, Debug (threads and frames), Breakpoints and Reports (build/run/test history with full logs). Xcode 27 dropped the Symbol navigator: the file's symbols are in Document Items (⌃6), the workspace's in Open Quickly. | ⌘1 … ⌘9 |
| **Source control** (git) | Change bars beside the line numbers for lines added (green), modified (blue) and deleted (a red wedge) against `HEAD`, drawn hollow once staged. They follow typing: the editor's text is compared with git's versions in the background, not the saved file. Click a bar for the change's old and new lines with Discard Change (one ⌘Z undoes it), Stage Change / Unstage Change and the next and previous change; Navigate ▸ Jump to Next / Previous Change. | ⌃\\ ⌃⇧\\ |
| | Inline blame (Zed's): the author, how long ago and the commit's summary after the cursor's line, a moment after the cursor rests (Editor ▸ Inline Blame turns it off). Editor ▸ Authors: each run of lines' author and date in a column of the gutter, with lines not committed yet marked; click for the commit's message and id. | ⌃⇧⌘A |
| | Editor ▸ Code Review: the file against `HEAD` in a tab, side by side (aligned, scrolling together) or inline (Editor ▸ Side by Side / Inline Comparison), its local side editable like the file's own tab. | ⌥⇧⌘↩ |
| | Integrate ▸ Commit…: the changed files with a checkbox for staged, the selected file's staged and unstaged changes with Stage / Unstage on each, the message (kept while the sheet is closed) and Amend. Push… (to the upstream or a new remote branch, Include tags) and Pull… (Rebase local changes onto upstream changes), Fetch Changes, Stage / Unstage / Discard All Changes, Switch Branch… and New Branch… (also from the status bar's branch). Open files without edits reload after a checkout or pull. git runs off the main thread and never prompts: credentials come from git's helpers or an SSH agent. | ⌥⌘C, ⌥⌘X |
| **Bookmarks** | Edit ▸ Bookmark Line and Bookmark File (⌥ adds a dialog for the description); a bookmark's description starts as the selection or the line's text. Bookmarked lines get an orange ribbon left of the line numbers (right-click for its menu) and follow edits, undo, renames and Move to Trash, as breakpoints do (undoing the deletion of a bookmarked line brings its bookmark back, in its place). Jump to Next / Previous Bookmark in the file, or In Workspace in the navigator's order. | ⌃⌘' ⌃⇧⌘' ⌃⌥⌘' ⌃⌥⇧⌘', ⌃; ⌃: ⌃⌥; ⌃⌥: |
| | The Bookmarks navigator lists them grouped by file: click to open, double-click (or Edit Description) to edit the description in place, Mark as Complete, drag to reorder bookmarks within a file or whole files, delete, Sort By Name / Type / Completion Status, Delete All Completed Items, a filter and Show only incomplete items. From the Find navigator, Bookmark Line on a result, Bookmark File on a file, and the whole query with its results (its bookmark button, or Bookmark Line while the Find navigator has focus) as a group that Find Again refreshes. Saved per project. | ⌘3 |
| **Build & run** | Scheme, configuration and run destination pickers. Destinations are grouped as in Xcode: My Mac, connected iOS, iPadOS, watchOS, tvOS and visionOS devices (from `devicectl`, with how they're connected or why they can't be used: not paired, Developer Mode disabled, unavailable), Any iOS Device (arm64) for building, then every available simulator, filtered by the scheme's `SUPPORTED_PLATFORMS`. Devices appear and disappear as they connect, without a refresh. | ⌃0, ⌃⇧0 |
| | Build, Run, Test, Stop and Clean. Files are saved first. | ⌘B, ⌘R, ⌘U, ⌘., ⇧⌘K |
| | Edit Scheme for the Run and Test actions: build configuration, Debug executable, arguments passed on launch and environment variables (each with its checkbox; `$(BUILD_SETTING)`s expand), working directory, launching without state restoration, and the Address, Thread and Undefined Behavior Sanitizers, Main Thread Checker, Malloc Scribble, Guard Edges and Stack Logging, and Zombie Objects. Changes are written in place to the scheme's `.xcscheme` (shared or the user's), the rest of the file left byte for byte; Swift packages keep them in zcode's session. Run… and Test… open the editor with a button that starts the action. Product ▸ Scheme also has Next and Previous Scheme. | ⌘<, ⌥⌘R, ⌥⌘U, ⌃⌘] ⌃⌘[ |
| | The scheme's options reach every launch: simulator apps (`simctl launch` with `SIMCTL_CHILD_` variables), Mac apps, package products and lldb-dap; sanitizers reach `xcodebuild` and `swift build`/`swift test`, tests build with the Test action's configuration, and the Test action's environment reaches `swift test` and the in-simulator runner. The toolbar's build configuration is the Run action's. Mac executables and package products run on a pseudo-terminal, as in Xcode's console, so their output streams as it is printed. | — |
| | Running on a simulator boots it, opens Simulator.app, installs and launches the app, and streams the app's stdout/stderr into the console. Mac executables and Swift packages run directly. | — |
| | Running on a device builds with automatic signing (`xcodebuild -destination id=… -allowProvisioningUpdates -allowProvisioningDeviceRegistration`), installs with `devicectl device install app`, waits while the device is locked ("Unlock it to continue"), launches with `devicectl device process launch --console` and the scheme's arguments and environment, and streams the app's output into the console. Stop terminates the app. A device that can't run apps says why before anything builds. Signing errors appear in the Issue navigator under the project, saying where in zcode to set the team; clicking one opens the project settings. ⌘U runs `xcodebuild test` on the device. | — |
| | Build output streams into the debug area as a condensed Xcode-style activity log, or as the raw `xcodebuild` log. Errors and warnings appear inline, in the Issue navigator and with next/previous issue keys. | ⇧⌘Y, ⇧⌘C, ⌘K, ⌘' ⌘" |
| | Test results from XCTest and Swift Testing populate the Test navigator, and Swift Testing failures appear as issues at the failing line. On a simulator, unit tests run inside the host app by injecting the test bundle (XCTest's own hosted-test mechanism). That is faster than `xcodebuild test` and doesn't depend on its test-runner service. | ⌘U |
| **Testing** | Test diamonds in the editor gutter on XCTest classes and `test…` methods, Swift Testing `@Suite` types and `@Test` functions (found in source as you type, Objective-C XCTest too). They show the last result; clicking one runs just that test or class. | — |
| | Test Selected Items runs the test (or class) at the cursor or selected in the Test navigator; Test Again repeats the last test run; Re-run Failed Tests runs what failed last; Clear Test Results. | ⌃⌥⌘U, ⌃⌥⌘G, ⌃⌥⌘K |
| | The Test navigator lists targets, classes, suites and tests from source with their results; each row runs on its play button or context menu. Results of tests a run doesn't include stay, as in Xcode. | ⌘6 |
| | Test plans: the scheme's `.xctestplan`s in Product ▸ Choose Test Plan… and the Test navigator; ⌘U and Test Again run the chosen plan, and tests it leaves out are dimmed. | — |
| | Every runner gets the selection: `xcodebuild test -only-testing:`/`-testPlan`, `swift test --filter`, and in the simulator an XCTest scope for the injected bundle (which also selects Swift Testing tests), honoring the plan's skipped and selected tests, environment and arguments. | — |
| **Debugging** (lldb-dap) | Breakpoints as blue markers on the line numbers: click the gutter to add one, click a marker to disable or enable it, double-click it (or Edit Breakpoint…) to edit it, right-click for its menu. They follow edits (lines inserted above move them), undo, renames and moves, and are saved per project; deleting a breakpoint's line deletes it, and undoing that brings it back. ⌘R runs under the debugger whenever enabled breakpoints are active, as in Xcode. Simulator apps launch with `--wait-for-debugger` and lldb attaches to them. Device apps launch suspended and lldb attaches through CoreDevice (lldb's `device select` and `device process attach`, as Xcode does), using the local build's symbols. | ⌘\\, ⌘Y |
| | The breakpoint editor popover: name, condition, "Ignore N times before stopping", actions (Log Message with `@expression@`, `%B` and `%H`, logged or spoken; Debugger Command; Sound) and "Automatically continue after evaluating actions". | — |
| | Symbolic (symbol and module), Swift Error and Exception (Objective-C, C++ or both, on throw or catch) breakpoints from the Breakpoint navigator's + menu or Debug ▸ Breakpoints; the navigator lists every breakpoint with its toggle and options. | ⌥⌘\\ |
| | Pause/continue, step over/into/out, the stopped line highlighted in green, threads and frames in the Debug navigator, and an expandable Variables view next to the console, with Copy Value and Print Description. Hovering a variable while paused shows its value. | ⌃⌘Y, F6, F7, F8 |
| | The LLDB console: an `(lldb)` prompt while paused takes `po`, `p`, `v`, `expr` and any other lldb command, with ↑/↓ history; an empty Return repeats the last command. | ⇧⌘C |
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
* sourcekit-lsp, scheme listing, simulator and device discovery, file
  scanning and git status all start after the window appears, on background
  threads. git status is read again after saves (debounced), git
  operations, window activation and changes in the git directory (a cheap
  poll of a few files' times); change bars come from a diff of the editor's
  text run in the background 30 ms after the last keystroke, so typing
  latency doesn't change (median keystroke to frame in a 6,000-line file:
  5.3 ms on this branch, 5.4 ms on `master`). Devices are listed again every 5 s, only while the project's
  window is active and its scheme builds for a device platform.
* Semantic tokens and inlay hints are asked for after typing pauses (300
  and 400 ms), a file's tokens whole and each pane's hints for its visible
  rows and a screen either side; a newer request cancels the one before and
  answers for text that has changed are dropped, so 30 keystrokes cost one
  semantic token request and a few hint requests. Typing in the middle of a
  6,000-line Swift file with both on: median keystroke to frame 5.0–5.1 ms,
  against 5.2–5.3 ms on `master` (alternating runs, load average 4–8). A
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
  with a Swift file's tab instead). Cold launch on this branch: median 247
  ms against 252 ms on `master` (alternating runs, load average 8–11).
* Vim mode costs nothing while it's off: the editor has no keystroke
  interceptor or observer then, and the status bar follows Vim through an
  event an editor only emits with it on. On, insert mode passes keystrokes
  through after a check, and what `.` repeats is worked out once, at
  Escape. Typing in the middle of a 6,000-line Swift file, four
  alternating rounds each: median keystroke to frame 3.8–5.2 ms with Vim
  mode off and 5.0–5.2 ms in insert mode (load average 6–13). Cold launch,
  alternating with `master` (load average ~7): the welcome window's first
  pixels at 247.0 ms against 248.4 ms (median of 24), and ZKit with two
  Swift tabs restored at 226.3 ms against 224.5 ms to the first frame
  (median of 20), 225.9 ms with Vim mode on.
* Settings cost nothing measurable at startup: the user's settings,
  keymap and the theme they name are read and parsed on a thread of their
  own started at the top of `main()`, and the first window takes what it
  read (~0.06 ms on the main thread); with no files it's one failed
  `open` each. A project's `.zcode/settings.json` is read when its window
  opens, and every file is polled off the main thread about once a second
  after that. Cold launch, alternating runs against `master` (load average
  9–17): the welcome window's first pixels at 253.2 ms with no settings
  files and 255.0 ms with settings, a custom theme, a font and key
  bindings, against 259.0 ms on `master` (median of 20); ZKit at 262.4 /
  265.2 ms against 262.7 ms (median of 12).
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
  workspace/        project window: buffers (one per open file, a linked editor per pane), panes of tabs in
                    a split tree with the assistant, navigators, inspector, debug area, pickers, commands;
                    source control (change bars, blame, Code Review, the commit and push/pull sheets);
                    language features (inlay hints per view, semantic tokens per file, the hierarchies
                    in the Find navigator); multibuffers (find results, issues, references, renames
                    as tabs of excerpts, with headless buffers for the files no pane shows); terminals
                    in the debug area and as pane tabs; Vim mode in every editor tab (`:w`, `:q`, the
                    status bar's mode); the settings applied to the window's editors, terminals and git
                    views, Settings… and the theme and import pickers
  lsp/              JSON-RPC client, document sync store, editor provider adapters, what the server offers;
                    inlay hints, semantic tokens and call/type hierarchies converted for the editor
  build/            xcodebuild/swift/simctl/devicectl queries, streaming runner, log/issue/test
                    parsing, compilation-database extraction
  packages/         Swift package dependencies: rules and versions, Package.resolved, checkouts,
                    Package.swift edits, git queries, resolve/update/reset
  testing/          tests found in source, test plans, test selections per runner, results → tests
  scheme/           Edit Scheme options: .xcscheme read and edited in place, what launches get
  plist/            property lists: XML with byte ranges, binary, in-place edits, Xcode's key definitions
                    (generated by scripts/plist_definitions.py)
  signing/          teams, identities and provisioning profiles; capabilities and where they live
  debugger/         DAP client, debugger model, breakpoints and their options, data tips (lldb-dap)
  terminal/         the integrated terminal: a PTY and its grid (alacritty_terminal), keystrokes and
                    mouse reports as bytes, links, theme colors, the view and how its grid is painted
  project.rs        project detection and gitignore-aware scanning
  xcodeproj.rs      project.pbxproj parser and in-place editor (build settings)
  xcodeproj/        structural edits: files, groups, build files, target membership, packages,
                    a target's Info.plist and entitlements
  git/              git through its command line: status, index and HEAD versions, hunks (line diffs)
                    and staging a hunk through the index, commits, blame, branches, fetch, pull, push
  search.rs         parallel find-in-workspace
  bookmarks.rs      bookmarks: lines, files and saved find queries, kept in the session
  settings/         settings.json (a project's over it), keymap.json and JSON themes: JSON with comments
                    read and edited in place, read off the main thread at launch and polled for changes;
                    the theme in effect; Xcode's .xccolortheme themes and .idekeybindings key bindings
                    converted (plist/ reads XML, binary and old-style property lists)
  perf.rs           startup timing and memory sampling (proc_pid_rusage)
  debug.rs          scriptable UI control channel for tests (ZCODE_DEBUG_DIR)
crates/
  zcode-editor/     the code editor: rope buffer, display map (soft wrap, folding), highlighting,
                    undo, multiple cursors, linked views of one buffer, multibuffers (excerpts of many
                    buffers, one undo step across them), Vim mode (a keystroke interceptor with
                    motions, text objects, operators, registers, search and `:` commands), LSP
                    popovers, find panel (docs/editor-engine.md)
vendor/
  gpui-component/   gpui-component 0.7.3 plus small patches (ZCODE-PATCH.md)
scripts/
  e2e.py            end-to-end UI tests
  appkit_floor.swift  minimal native AppKit app, the cold-launch reference
```

The code editor is zcode's own crate, `crates/zcode-editor`. It started as
the text engine of `gpui-base` and the editor UI of `gpui-component` (both
Apache-2.0, from GPUI Kit 0.7.3; see its `NOTICE`), and zcode now evolves it
(`docs/editor-engine.md`): linked views, which let two panes show one
buffer, and multibuffers, whose excerpt model, headers above rows and undo
across files were written from the public description of Zed's (Zed's
editor is GPL; none of its code was read). The workspace's panes, items and split tree follow
the model Zed describes publicly (a pane of items, a tree of splits), written
from scratch. The rest of `gpui-component` (widgets, theme,
tree-sitter highlighter) is vendored with a startup patch and a visibility
patch for the highlighter, both described in
`vendor/gpui-component/ZCODE-PATCH.md`.
Source control runs the `git` command line; line diffs come from the
`similar` crate (Apache-2.0). Inline blame and hollow staged change bars
follow Zed's public behavior, implemented from scratch, as do inlay
hints (off by default, refreshed for what's on screen), signature help and
the call hierarchy's outgoing calls; the hierarchies' place in the Find
navigator and their command names are Xcode's. Multibuffers follow Zed's
public behavior too (excerpt headers, ⌥↩ to open an excerpt's file, ⇧↩ to
expand it, editing search results to replace), next to Xcode's Find
navigator and its Find and Replace in Workspace command.
The integrated terminal uses `alacritty_terminal` (Apache-2.0) for its
PTY, VT parser and grid with scrollback, which brings `vte` (Apache-2.0
OR MIT), `signal-hook` (MIT OR Apache-2.0) and `rustix-openpty`
(Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT); its view, painting,
key and mouse encoding and links are zcode's. Its place beside the
console and in editor tabs follows Zed's public behavior (Zed's terminal
is GPL; none of its code was read), and Open Terminal at Location is
Xcode's command.
Vim mode is written from Vim's documented behavior (`:help` for motions,
operators, text objects, registers and patterns), with Zed's public
behavior where Vim leaves a choice to the host: `:q` closes a tab, the
unnamed register is the system clipboard, a selection made without Vim's
keys is visual mode, and Escape closes popovers before it leaves insert
mode. Xcode's own Vi bindings (`IDEViEventConsumer`) live in code, not in a
data file, so Editor ▸ Vim Mode's name and place are what zcode takes from
it. No Vim or Zed code was used.
The settings files follow Zed's public documentation (file names and
places, JSON with comments, setting names and shapes such as
`buffer_font_size` and `inlay_hints.enabled`, the keymap's blocks,
`context`, `null` and `zed::Unbind`); their reader, in-place editing,
merging and reloading are zcode's own (Zed's settings and keymap code is
GPL; none of it was read). Themes use gpui-component's theme format
(Apache-2.0), whose editor part is Zed's theme format, so Zed's theme
files load too. The Xcode importers read Xcode's own files
(`.xccolortheme` and `.idekeybindings` property lists, old-style ones
included), and map its commands to zcode's by the action selectors in
Xcode's command definitions; Settings… is Xcode's command and shortcut.
zcode doesn't use any GPL code from Zed.

## Testing

```
cargo test                          # unit tests (zcode and crates/zcode-editor)
cargo test -- --ignored             # needs Xcode: sourcekit-lsp, lldb-dap, and xcodebuild on an edited project
                                    # (network_* tests need network access)
cargo build --release && python3 scripts/e2e.py [--simulator] [--network] [--compare-apps]   # drives the real UI
```

`scripts/e2e.py` launches zcode with `ZCODE_DEBUG_DIR` set and checks against
a temporary copy of `samples/` taken from git `HEAD`, so local edits and
saved breakpoints can't leak in. Every zcode it starts gets a temporary
`ZCODE_CONFIG_DIR`, which stands in for `~/.config/zcode` and the session's
folder, so a run never reads or changes your settings, key bindings,
themes or session. It covers:
* Open Quickly, definition, completion with placeholders, comment and undo, Document Items, find and references
* project settings: a General field edited and restored, with
  `project.pbxproj` checked byte for byte
* property lists: Xcode's key and value names, raw keys, key suggestions,
  add, retype, move, ⌘Z/⇧⌘Z with the dirty dot, ⌘S (byte for byte when
  unmodified), Open As; a binary plist saved as binary; privacy manifest
  and entitlements names
* the Info tab (an `INFOPLIST_KEY_` setting edited and restored byte for
  byte; the first key creating `Info.plist` with its synchronized-folder
  exception) and Signing & Capabilities on copies of HelloZ (App Groups,
  Push, iCloud, Background Modes, then a ⌘B build signed with them) and
  HelloClassic (App Sandbox, Hardened Runtime, App Groups with Sign to Run
  Locally, a ⌘B build, then removing the sandbox)
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
* editor panes on a copy of ZKit with an Objective-C pair and a property
  list: ⌃⌘T / ⌃⌥⌘T splits, two panes on one buffer (typing in one, ⌘Z and
  ⇧⌘Z in the other, a cursor each), focus with ⌃\` ⌃⇧\` and clicks;
  breakpoints, bookmarks and test diamonds in every pane through edits and
  undo made in another; a property list edited in one pane and undone in
  the other; the assistant's Counterparts, ⌘↩, ⌥-Return, ⌥-click, ⌃⌥⌘↑
  and ⌃⌥⌘J; Focus Editor Pane, Close Other Tabs, ⌃⇧⌘W, ⌘W on a pane's last
  tab; a package source read-only in a split; a dragged split, the
  layout, tabs, assistant and focus after a relaunch
* the editor's extension points (`crates/zcode-editor`) on a copy of ZKit:
  inlays the caret steps over and that follow typing and undo, blocks that
  move the rows below them, a highlight layer following edits, and a
  keystroke interceptor with a block caret; package sources refusing the
  editing commands and undo
* language features on a copy of ZKit with a class hierarchy, a loose C
  file and a 6,000-line Swift file, against the real sourcekit-lsp: inlay
  hints after bindings and clangd's parameter names before C arguments,
  refreshed after typing and gone when turned off; types at calls from
  semantic tokens, operators left to tree-sitter; signature help from `(`
  until the call closes or Escape; Find Call Hierarchy from the second of
  two panes (callers, callees, opening calls and declarations) and
  ancestor, descendent and conforming types; typing latency with both on.
  Then through `scripts/lsp_proxy.py` (the real server behind
  `ZCODE_SOURCEKIT_LSP`, logging what zcode asks): the requests typing and
  scrolling cost, and a server with these capabilities hidden (nothing
  asked for, ⌃⇧⌘H says so, completion still works)
* package dependencies, offline against a local repository of the Tally
  fixture (`tests/fixtures/packages`): Add Package Dependencies with its
  tags, rule and products, Add Local…, a build against both packages,
  browsing a package and Jump to Definition into it (read-only), Resolve
  keeping the pin and Update Package moving to a new release, a resolve
  error under Package Loading, and Remove Package; in a Swift package
  (`Package.swift` through `swift package`) and an `.xcworkspace`.
  `--network` also lists a public package's versions from GitHub
* source control on a copy of ZKit made a new repository, with a local
  bare repository as `origin` (git runs with the global configuration out
  of the way): change bars for typed edits before a save, a bar's popover
  with Discard Change, ⌘Z and Stage / Unstage Change (checked in the index),
  ⌃\\ and ⌃⇧\\, inline blame and Authors with its commit popover, Code
  Review side by side and inline with an edit on its local side, Commit…
  with one hunk staged and then Amend, New Branch, Push… to a new remote
  branch, Switch Branch, Fetch Changes and Pull… from another clone's
  push, a commit made in a terminal, and Discard Changes
* multibuffers on a copy of ZKit made a repository, with 400 text files:
  a rename through sourcekit-lsp shown in a tab and undone with one ⌘Z;
  Find results in a tab (headers, each line's number in its file, match
  highlights); typing in an excerpt reaching the file and back, undone from
  the file's tab; Find and Replace in Workspace's Replace All across four
  files, undone and redone in one step, one file's part undone from its own
  tab, saved with ⌘S; ⌥↩, a header and a line number opening the file;
  ⇧↩ and a header's ↕; change bars and a breakpoint toggled on excerpt
  lines; Show Issues in Editor after a failed build, fixed in place; a
  search over 400 files opening a page of 25 and adding more as it scrolls,
  typing latency there and on Swift excerpts, and the files let go when the
  tab closes
* the integrated terminal on a copy of ZKit, with `zsh -f` as the shell
  (`ZCODE_TERMINAL_SHELL`) so the user's configuration can't change what
  it prints: nothing started at launch, ⌃⌘\` opening a shell in the
  project's folder at the grid's size, ⌃C, ⌃\\ and ⌃6 reaching the shell
  and ↑ its history, ⌘K, a dragged selection and ⌘C, a double-clicked word,
  ⌘V, the wheel through the scrollback, ⌘-click on `file:line:column` and
  on an OSC 8 link, a bigger font giving the shell fewer columns (`stty
  size`), several terminals in the debug area and one in a tab, Open
  Terminal at Location, `yes` flooding a terminal while typing in an editor
  pane, ⌘W and `exit` closing terminals and ending their processes, and
  quitting and relaunching (fresh shells in the folders they were in)
* Vim mode on a copy of ZKit: off by default, then Editor ▸ Vim Mode on in
  every tab; `ciw`, `.`, `D`, `d2w`, `yy p` and `u` through the real key
  path (one undo step each); `/` search, `V j >` and a visual selection;
  `:w`, `:q` refusing unsaved changes, `:q!` and `:wq` closing the tab and
  not the window; Escape closing sourcekit-lsp's completion before it
  leaves insert mode; `:w` in a multibuffer saving the file it changed;
  Open Quickly's field, a terminal and a property list outline left
  alone; a mode per pane; typing latency in insert mode against Vim mode
  off; the setting after a relaunch. The keystroke sequences themselves
  are unit-tested headless in `crates/zcode-editor/src/vim/tests.rs`
* settings, key bindings and themes on a copy of ZKit, in the run's own
  configuration folder: ⌘, opening `settings.json`; tab size, hard tabs,
  font size, wrap, invisibles and line numbers applied as soon as the file
  is saved; a file with a mistake keeping its last good settings and saying
  where; a project's `.zcode/settings.json` over it (not for the theme or
  Vim mode); the Editor menu's toggles and ⌘+ writing the file, with their
  checkmarks; trailing whitespace, final newline and format on save through
  sourcekit-lsp; `terminal.shell`; `keymap.json` with an `Editor` binding,
  one for Vim's normal mode, one only in the Find navigator, Unbind and a
  mistake reported; themes in gpui-component's and Zed's formats, edited
  live, a missing one, and Select Theme…; an Xcode theme imported from a
  fixture and Xcode's own Midnight read in place; a fixture's Xcode key
  bindings imported (a rebound command, a removed one, a text binding, and
  again in place) and your own sets read; the default files read-only; the
  session's old toggles moved into `settings.json` once; and
  `~/.config/zcode` left as it was
* bookmarks on a copy of ZKit: Bookmark Line and File, ribbons, edits and
  undo (a deleted line's bookmark back with ⌘Z), next and previous in the file and the workspace, descriptions
  edited in place, completion, the filter, drag and drop (real mouse
  events), the With Dialog variants, the ribbon's menu, a Find query and a
  result with Find Again, renames, Move to Trash and a relaunch
* breakpoints in a Swift package: gutter clicks, line tracking through edits
  and undo (deleted with their line, back with ⌘Z), the editor popover; a condition, an ignore count, log and command
  actions with auto-continue; a symbolic breakpoint in and out of its module;
  Swift Error and Objective-C exception breakpoints; `po`/`p`/`expr`/`v`/`bt`
  at the console's prompt with history, Print Description and data tips;
  breakpoints restored after a relaunch. A Mac app (`samples/HelloClassic`)
  stopped under lldb-dap with the console and an auto-continuing log message
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
  and debug on an iOS simulator (with the LLDB console)

The control channel also takes window screenshots
(`screenshot <path>`), which doesn't need Screen Recording permission.

## Known limitations

* Project settings cover the General fields, Signing & Capabilities, Info
  and build settings. Adding or removing targets or configurations still
  needs Xcode, as do the Resource Tags, Build Phases and Build Rules tabs.
* Signing & Capabilities sets signing for all configurations at once (no
  Debug/Release split) and offers the capabilities listed above, not
  Xcode's whole library (no Apple Pay, Game Center, Wallet, Siri, Network
  Extensions, …: add their entitlements in the `.entitlements` file's
  property list editor). Capabilities that need Apple's servers are
  registered by `xcodebuild -allowProvisioningUpdates` at the next device
  build, not when they're added; the team list needs Xcode's account or a
  signing identity in the keychain, otherwise type the team ID. Profiles
  can't be downloaded or imported from zcode.
* The property list editor has no cut, copy or paste of rows (Open As ▸
  Source Code for that), shows dates as `YYYY-MM-DDTHH:MM:SSZ` in UTC and
  data as hex, and only reorders rows within their container. Keyed
  archives' UIDs read as `CF$UID` dictionaries and are saved that way. The
  Info tab shows `INFOPLIST_KEY_` settings as text (lists space-separated)
  and doesn't show the keys Xcode derives from other build settings
  (bundle identifier, version).
* Packages: the search field takes a URL or a GitHub `owner/repo`; there's
  no package collections search, registry packages can't be added (their
  pins are shown), and a package's dependency rule can't be edited after
  it's added (adding it again only adds products). Package plugins aren't
  offered. `xcodebuild` has no update command, so Update Package and Update
  to Latest Package Versions drop the pins and checkouts concerned and
  resolve again (the repository caches stay). Remove Package leaves empty
  `packageReferences` lists, as Xcode does; in a Swift package it removes
  the `.package` entry and `.product(…, package:)` uses, but not uses by
  product name alone. Local packages that pre-Xcode 15 projects reference
  as folders in the group tree aren't listed, and packages a workspace
  lists directly can't be removed from zcode. Private repositories work
  through git's credential helpers; zcode never prompts for credentials.
* In file-system-synchronized folders (the Xcode 16+ default) files belong to
  the folder's targets; zcode doesn't edit their per-file exceptions, so the
  File inspector shows that membership read-only. Trashing a whole
  synchronized folder leaves its group in the project.
* Structural edits don't create variant groups (localized `.lproj` folders
  are added as folder references) or `XCVersionGroup`s (`.xcdatamodeld` is
  added as a plain file reference), and Headers-phase visibility (Public,
  Private) isn't editable. The navigator has no drag and drop: move items by
  renaming them to a path.
* Bookmarks are grouped by file rather than in Xcode's free-form groups:
  there's no New Group, Cut/Copy/Paste or Duplicate Bookmark, and a
  bookmark can't be dragged into another file's section. Edits made outside
  zcode don't move bookmarks. They live in zcode's session, so Xcode doesn't see them.
  A bookmarked search keeps one bookmark per matching line, and Find Again
  searches the files of the last project scan.
* Source control: change bars compare with `HEAD` (staged changes drawn
  hollow), not with a chosen commit or branch. Code Review compares the
  file with `HEAD` only (no staged-only view, no other revisions or
  history), highlights whole lines rather than the words changed, and
  lines deleted before a file's first line show under that line (blocks
  can't go above the first row yet). The commit sheet has no file
  filter, no per-line staging (per change only) and no author override;
  Push… pushes the current branch; Pull… merges or rebases, and conflicts
  are left for a terminal or Xcode (marked `!` in the navigator). No
  stash, cherry-pick, tags, history inspector, clone, or merge conflict
  editor. git never prompts, so a remote that wants a password fails unless
  a credential helper or SSH agent has it. A package checkout (its own
  repository) gets no change bars.
* Editor panes: the assistant has Counterparts and Manual modes only (no
  Callers, Superclasses, Preview or other Xcode modes), and Swift files have
  no counterparts (Xcode shows generated interfaces). Move Focus to Editor
  Pane… (⌘J) and the ⌥⇧ destination chooser aren't there; ⌥-click opens in
  the assistant or the next pane. Panes aren't dragged between windows, and
  tabs can't be dragged between panes. Project Settings is one tab, in one
  pane at a time, and isn't restored with the session.
* Language features come from sourcekit-lsp: the hierarchies come from
  its index, so they're empty until a new project's background index is
  built (the Find navigator says so) and cover what the index has seen;
  SDK symbols open at the top of their `.swiftinterface` (read-only).
  sourcekit-lsp gives Swift no parameter-name hints (Swift writes argument
  labels already). Signature help shows the active signature (no switching
  between overloads), and semantic token colors follow the theme's names
  (the default themes color types, functions, properties and constants,
  not Xcode's project-versus-system distinction). Documents are still sent
  whole on each change (no incremental sync), which completion's requests
  do on every keystroke.
* Multibuffers: excerpts are highlighted in the language most of their
  files are in, and have no completion, Quick Help, Jump to Definition or
  inlay hints (⌥↩ opens the file, where they work); bookmarks, test
  diamonds and blame aren't shown on excerpt lines. ⌘Z skips the part of a
  step in a file edited since from elsewhere (that file keeps it). Issue
  and find tabs show what they were opened with: Refresh (issues) or
  opening the results again (find) shows the current ones, and the issues
  tab says when they changed. The tabs aren't restored with the session.
  Find and Replace in Workspace replaces all results at once (no Replace
  for one result, no preserved case); review and undo it in the tab.
* The terminal has no input method composition (dead keys and IME), takes
  ⌥ as macOS composes it rather than as Meta (except ⌥← and ⌥→, which
  move by word as in Terminal.app), and has no find in its scrollback,
  settings of its own (shell, font, colors, scrollback size) or blinking
  cursor. A restored terminal is a new shell in the old folder: what ran
  in it and its scrollback are gone. Terminals can't be moved between the
  debug area and editor tabs. URLs open in the browser through macOS; the
  e2e checks don't open one (their detection is unit-tested), and Open
  Terminal at Location is driven through the same command its menu item
  calls rather than a click on the context menu.
* Vim mode has no macros (`q`, `@`), sentence motions and objects (`(`,
  `)`, `is`, `as`), `gq`, `!` filters, `:g`, `:normal`, `:sort` or `:set`,
  and `:s` has no `c` (confirm) flag. Marks are per tab and follow edits
  (uppercase marks too: they don't jump between files); `⌃O` and Tab are
  zcode's Go Back / Go Forward rather than Vim's jump list. `=` asks
  sourcekit-lsp to re-indent, so it does nothing without it. An insert in
  a multibuffer undoes a typed run at a time (the multibuffer keeps its own
  undo steps). Vim's own keys can be overridden in `keymap.json` (contexts
  with `vim_mode`), but not remapped one to another (no `:map`).
* Settings are files only: there's no settings window, and only the
  settings listed above (Zed has many more; others are reported as
  unknown). Files are checked for changes about once a second. A
  project's file can't change the theme, font family or Vim mode (they're
  the app's). Format on save applies to ⌘S, Save All and `:w`, waits at
  most 1.5 s for sourcekit-lsp, and doesn't run for the saves before a
  build. Themes color zcode's window and editor; an imported Xcode theme
  colors the editor (Xcode's themes don't cover the rest of the window)
  with the SDK's colors for names (tree-sitter can't tell a project's
  types and functions from the SDK's, as Xcode's colors do), without its
  font sizes, line spacing, console or markup colors, and Xcode's SF Mono
  isn't installed for other apps, so its themes keep zcode's font. The key
  binding import skips commands zcode doesn't have (and says which) and
  Xcode's sub-word and mark text commands. Menu checkmarks follow the
  window in front.
* No Interface Builder, asset-catalog editor, SwiftUI canvas or Instruments.
* Devices: there's no Devices and Simulators window (⇧⌘2), so pairing a new
  device, turning on Developer Mode and copying a device's system symbols
  still happen in Xcode. Without those symbols for the device's OS build
  (Xcode copies them to `~/Library/Developer/Xcode/iOS DeviceSupport` the
  first time it prepares a device; CoreDevice offers no command-line way),
  lldb reads system libraries from the device: attaching over the network
  takes about a minute, system frames show addresses rather than names, and
  expressions (`po`) are slow. Debugging needs an lldb with the `device`
  commands (Xcode 27's has them). Main Thread Checker
  isn't inserted on devices. Run, debugging, tests and signing were verified
  on an iPhone connected over the network; USB connections and iPad, Apple
  Watch, Apple TV and Vision Pro hardware were only exercised against
  recorded `devicectl` output and a fake `devicectl`.
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
* Breakpoint actions run when lldb reports the stop to zcode, so a
  breakpoint that auto-continues still pauses the program briefly (as
  lldb-dap's own log points do). Swift Error breakpoints can't be limited to
  one error type, exception breakpoints have no condition (as in Xcode), and
  there are no Test Failure, Runtime Issue or Constraint Error breakpoints,
  watchpoints, or breakpoint sharing (zcode keeps breakpoints in its session,
  not in the project's `xcuserdata`, so Xcode doesn't see them). Edits made
  outside zcode don't move breakpoints. The console takes lldb commands only while paused;
  a running program's stdin isn't connected.
* UI test bundles are skipped by the in-simulator unit-test runner and still
  need `xcodebuild test` (running only UI tests from a diamond uses it). On this machine that fails ("The test runner hung
  before establishing connection") because of the simulator runtime, with
  either installed Xcode, independent of zcode.

# zcode

A fast, Zed-inspired editor for Apple-platform development, written in Rust on
[GPUI](https://gpui.rs) (Zed's GPU UI framework) via
[GPUI Kit](https://gpui-kit.com). It edits, builds, runs, tests and debugs
Xcode projects and Swift packages without opening Xcode. It uses the
toolchain Xcode installs: `xcodebuild`, `simctl`, `sourcekit-lsp` and
`lldb-dap`. The web app an Apple app ships with (a Vite app in `web/`, the
pages of a web view) is edited in the same window, with the web's language
servers and Emmet.

```
cargo build --release
./target/release/zcode path/to/App.xcodeproj   # or .xcworkspace, Package.swift, a folder, a file
```

Requirements: macOS with Xcode installed (`xcode-select -p` pointing at it), Rust 1.90+.

As a Mac app:

```
python3 scripts/package.py bundle          # target/release/bundle/zcode.app, from the release build
python3 scripts/package.py sign --adhoc    # or sign (Developer ID, ZCODE_SIGN_IDENTITY) and notarize (ZCODE_NOTARY_PROFILE)
open target/release/bundle/zcode.app
```

Then zcode ▸ Install CLI puts a `zcode` command in `/usr/local/bin`, zcode ▸
Open at Login starts the resident instance (`--background`) when you log in,
zcode ▸ Check for Updates… installs a newer release, and Finder's Open With
lists zcode for `.xcodeproj`, `.xcworkspace`, Swift sources and folders.
`scripts/package.py release` does bundle, sign, notarize, zip, dmg and the
update feed in one go (`--dry-run` prints what it would run).

## What works

| Area | Feature | Keys (Xcode-compatible) |
|---|---|---|
| **Projects** | Opens `.xcodeproj`, `.xcworkspace`, Swift packages and plain folders. Includes a welcome window with recent projects and per-project session restore (panes and tabs, scheme, destination, breakpoints, layout). | ⌘O |
| | Project settings: click the project in the navigator to edit the project and its targets (General fields such as bundle ID, version, deployment targets and team, plus every build setting per configuration). Edits are written straight to `project.pbxproj` and keep its formatting. The panel can also open `project.pbxproj` as text or open the project in Xcode. | — |
| | Signing & Capabilities for each target: Automatically manage signing, Team (from Xcode's signed-in accounts and the keychain's signing identities, or typed), Bundle Identifier, Signing Certificate (Development, Distribution, Sign to Run Locally, or a keychain identity) and Provisioning Profile (installed profiles that match the team and bundle ID). + Capability adds App Groups, iCloud, Push Notifications, Keychain Sharing, Background Modes, Associated Domains, Sign in with Apple, HealthKit, and on the Mac App Sandbox and Hardened Runtime, each with its options: entitlements go into the target's `.entitlements` file (created, set as `CODE_SIGN_ENTITLEMENTS` and added to the project the first time), Background Modes into its Info.plist, App Sandbox and Hardened Runtime into build settings, as Xcode 15+ keeps them. Builds register them with Apple through `-allowProvisioningUpdates`; zcode never calls the developer portal. Signing errors open this tab. | — |
| | The target's Info tab: Custom Target Properties, its Info.plist keys and `INFOPLIST_KEY_` build settings together in the property list editor. The first key added to a target with a generated Info.plist creates the file next to its sources and sets `INFOPLIST_FILE` (in a synchronized folder, with the membership exception that keeps it out of the target's resources). | — |
| **Property lists** | `Info.plist`, `.entitlements`, `.xcprivacy`, `.stringsdict` and any binary or XML `.plist` open in a Key / Type / Value outline: disclosure for dictionaries and arrays, Xcode's names for keys and values ("Bundle display name", "Privacy - Camera Usage Description", "Portrait (bottom home button)") from Xcode's own definitions, Editor ▸ Raw Keys and Values for the raw ones, + and − on each row, Editor ▸ Add Item and Value Type (Array, Dictionary, Boolean, Data, Date, Number, String), key suggestions as you type, menus for Booleans and allowed values, and rows moved with ⌥⌘[ ⌥⌘] or by dragging. Every change edits the file's text in place, so undo, redo, the dirty dot and ⌘S work as in source code, an unmodified file saves byte for byte, and new values are written as Xcode writes them (tabs, the `<!DOCTYPE>` header). Binary plists are saved as binary, keys in their order. Open As ▸ Source Code / Property List (Editor menu, or the navigator's context menu) switches views. | ⌥⌘[ ⌥⌘], ⌘Z |
| **Asset catalogs** | An `.xcassets` folder is one item in the project navigator, as in Xcode, and opens in Xcode's asset catalog editor as a tab (opening one of its sets' folders, as MCP's `OpenFile` or the control channel can, opens the catalog on that set): the outline of image, app icon, color, data and symbol sets and their folders on the left, the selected set's wells in the middle in rows by device and appearance ("Universal · Dark", "iOS"), each captioned with its scale, size or appearance ("2x", "16pt 1x", "Tinted"), with thumbnails drawn from the files (PNG, JPEG, HEIC, PDF, SVG…) and color swatches from `Contents.json`; a folder shows its sets as tiles. The attributes inspector: name, type, properties (Render As, Preserve Vector Data…), a well's file, pixel size and file size, files no slot names, and for a color its Content (color space), Input Method (floating point, 8-bit, 8-bit hexadecimal) and components. Edits: drop images from Finder into a well (copied into the set and named in `Contents.json`; the file the slot had goes unless another slot keeps it) or onto the outline or a folder (a new image set per name, `name@2x.png` in the 2x well), Delete clears a well; + New Image Set, Color Set, Folder and Import…; − or Delete removes a set; rename in the Name field; type a color's components, change its content or input method. `Contents.json` is written as Xcode writes it, byte for byte. Every edit is undone with ⌘Z (and redone with ⇧⌘Z), removed files included; `project.pbxproj` never changes (Xcode references the catalog as one folder), and new files appear in synchronized folders as they are. | ⌘Z ⇧⌘Z, ⌫ |
| **SwiftUI previews** | A mark over the line number on each `#Preview` (named, with traits, `arguments:`, a widget's timeline, a view controller) and `PreviewProvider`, found in source as you type, as test diamonds are, in every pane. Clicking one, or Editor ▸ Show in Xcode Canvas (the preview at the cursor, else the file at the cursor's line), saves the file and shows it in Xcode's canvas: Xcode opens the project, then the file at that line in the project's window, and Editor ▸ Canvas is turned on (and refreshed when it waits paused). Turning the canvas on needs zcode allowed under Privacy & Security ▸ Accessibility; without it the file opens and zcode says to press ⌥⌘↩. zcode doesn't draw previews itself: [`docs/previews.md`](docs/previews.md) describes the bridge and measures how far a native preview on the simulator could go. | ⌥⌘↩ |
| **Swift packages** | Package Dependencies at the bottom of the project navigator: every resolved package with its version (or branch, or commit), expandable to browse its sources, which open read-only (with a lock on the tab), as do package sources Jump to Definition reaches. | — |
| | File ▸ Add Package Dependencies…: enter a repository URL or a GitHub `owner/repo`; its versions come from its tags (`git ls-remote`) for the dependency rule (Up to Next Major Version, Up to Next Minor Version, Range of Versions, Exact Version, Branch, Commit). Add Package fetches that version to read its products; then pick the target each product is added to. Add Local… adds a package folder. In an `.xcworkspace` you choose which project gets the package. | — |
| | File ▸ Packages ▸ Reset Package Caches, Resolve Package Versions, Update to Latest Package Versions; Update Package and Remove Package on a package's context menu. Xcode projects are edited in `project.pbxproj` as Xcode writes it (package references, product dependencies, the targets' Frameworks phase) and resolved with `xcodebuild -resolvePackageDependencies` into the DerivedData folder builds use; Swift packages use `swift package add-dependency`, `add-target-dependency`, `resolve`, `update` and `reset`. Progress shows in the activity view, errors under Package Loading in the Issue navigator, and the index refreshes afterwards so new modules can be imported. | — |
| **Editor panes** | File ▸ New ▸ Editor Pane on Right / Below splits the focused pane, showing its file at the same place; panes split further side by side or stacked and are resized by dragging. Each pane has its own tabs, jump bar and Go Back / Go Forward history. A file shown in several panes is one document: an edit, undo or redo in one pane shows in the others at once, each pane keeps its own cursor and scroll position, and breakpoints, bookmarks, test diamonds, issues, the execution mark and the property list outline appear in every pane. Close Editor Pane, Close Other Editor Panes, Close Other Tabs; closing a pane's last tab closes the pane. Move Focus to Next / Previous Editor Pane, or click; Focus Editor Pane shows the focused pane alone. The layout, tabs, sizes and focus are restored with the session. | ⌃⌘T ⌃⌥⌘T, ⌃⇧⌘W ⌃⌥⇧⌘W ⌥⌘W, ⌃\` ⌃⇧\`, ⌃⇧⌘↩ |
| | The assistant (Editor ▸ Assistant) opens beside the focused editor in Counterparts mode, following it with the `.h` / `.m` (`.c`, `.cpp`, `.mm`) counterpart of its file, or in Manual mode with what was opened in it. ⌥-click in the project navigator, ⌥-Return (or ⌥-click) in Open Quickly, Jump to Definition and Jump to Next Counterpart with ⌥ open in the assistant (or the next pane, or a new assistant). Show Editor Only closes it. | ⌃⌥⌘↩, ⌘↩, ⌃⌥⌘J, ⌃⌥⌘↑ |
| **Terminal** (from Zed) | Terminals in the debug area's Terminal tab, beside the console (several at once, each with its tab and + for another), and in editor panes (File ▸ New ▸ Terminal Tab): the login shell on a pseudo-terminal, started in the project's folder. File ▸ Open Terminal at Location (also on the navigator's context menu) opens one in the selected item's folder. The editor's font and size, the 16 ANSI colors from the theme (so light and dark both read), 256 and 24-bit color, bold, italic, underlines; 10,000 lines of scrollback (the wheel, ⇧PageUp/PageDown); drag to select (double-click a word, triple-click a line, ⇧-click to extend), ⌘C, ⌘V (bracketed when the program asks), ⌘A; ⌘K clears it. ⌘-click a `file:line:column` (compiler errors, `grep -n`, test failures) to open the editor there, a URL to open the browser, or a link a program marks (OSC 8). Programs that ask get the mouse (`vim`, `htop`), and full-screen ones get the wheel as arrows. Plain and ⌃ keys go to the shell (⌃C, ⌃\\, ⌃6 included), ⌘ keys stay zcode's (⌘W closes the terminal). A tab is named after what's running in it. Closing a terminal, its tab or the window ends its processes; the session reopens terminals as fresh shells in the folder each was in. | ⌃⌘\`, ⌘K, ⌘W |
| **Coding assistant** (ACP, from Zed) | View ▸ Show Coding Assistant (⌘0, as in Xcode 27) opens the agent panel beside the inspector: a conversation with Claude Code, Codex or Gemini CLI (their Agent Client Protocol adapters, run with `npx`) or any agent you add to `agent_servers` in `settings.json`. Answers and thinking stream in as Markdown; tool calls show what they read and run, their output and their diffs; an agent that asks permission waits for Allow, Always Allow or Reject. Agents read files through zcode, so they see unsaved edits; what they write lands in the file's buffer as one undoable edit and is saved (⌘Z in any pane takes it back), and a change an agent makes on disk itself is taken into the buffer the same way. The files a conversation changed are listed with their lines added and removed: Review compares one with what it was before (as Code Review does with `HEAD`), Reject puts it back, Keep stops listing it. `@` (or the @ button) mentions a file or a symbol from sourcekit-lsp, sent with its text from the buffer. Follow Agent opens the files the agent reads and changes at the line, leaving the keyboard in the panel. The agent's plan, its modes (Always Ask, Plan…), Stop, signing in, and File ▸ New ▸ Conversation (⌃⌘H). Nothing loads until the panel is opened, and no agent starts until a message is sent. Show/Hide Navigator, which had ⌘0, has no key now, as in Xcode 27 (`keymap.json` can give it one). | ⌘0, ⌃⌘H, Return, ⇧Return |
| **MCP server** (for coding agents) | `zcode mcp` is a Model Context Protocol server on stdio for Claude Code, Codex, Gemini CLI, Cursor, Zed or any MCP client: it connects to the running zcode and acts on the open project, so an agent builds, runs and tests with your scheme and destination, reads the Issue navigator, the build log and the console, opens files, reads the editor's selection (unsaved edits included), evaluates in the paused debugger and takes screenshots of the window or a simulator. Results come back structured (each test's result, each error's file and line); builds and tests report progress and stop when the request is cancelled. The tools have Xcode 27's names where its own MCP server has the tool (`BuildProject`, `RunAllTests`, `GetBuildLog`…). The agent panel hands every agent it starts this server; the tools that build, run, stop, test or evaluate ask in the panel first, like the agent's own tool calls. See [zcode as an MCP server](#zcode-as-an-mcp-server). | — |
| **Predictive completion** (from Zed and Xcode) | Code predicted at the cursor, shown in gray as you type: the rest of the line where the text goes and the lines after it below, the text after the caret moved aside; Tab accepts it as one ⌘Z, Escape or ⌥] dismisses it, and typing what it predicts keeps the rest. Editor ▸ Show Predictive Completion (⌃⌥Space, as in Xcode) asks at once. Off until `edit_predictions.provider` in `settings.json` names one: `"claude"`, the Claude API (the model, `claude-haiku-5-5` by default, and the key from the settings or `ANTHROPIC_API_KEY`), streamed, sent the code around the cursor and your latest edits; or `"lsp"`, the language server's `textDocument/inlineCompletion` where it has one (sourcekit-lsp doesn't yet). Asked once typing pauses (`debounce_ms`, or only on ⌃⌥Space with `automatic` off); a newer request or a moved caret cancels the one before. The status bar shows what it's doing, errors included, and its menu turns predicting while typing on and off. Never in Vim's normal mode, read-only files, multibuffers, terminals or files matching `disabled_globs` (`.env`, keys and certificates by default). | ⌃⌥Space, Tab, Esc, ⌥] |
| **Editing** | Rope-based editor with tree-sitter highlighting (Swift, C/ObjC, C++, JSON, YAML, Markdown, and the web's: HTML, CSS, JavaScript, TypeScript, JSX/TSX, Vue, Svelte, Astro…), multiple cursors, folding, indent guides and find/replace. Snippets from completions (and Emmet) select their first tab stop; Tab moves through the rest. | ⌘F, ⌥⌘F |
| **Multibuffers** (from Zed) | One editor showing excerpts of many files, edited in place: Find ▸ Show Find Results in Editor (or the Find navigator's button) for Find in Workspace's results or Find Selected Symbol's references, Navigate ▸ Show Issues in Editor (or the Issue navigator's button) for every issue with its lines around it and its message under it, and a rename's changes. Each file has a header with its name and folder; the gutter shows each line's number in its file, its change bars and breakpoints (click one to toggle it), and the excerpts show the file's diagnostics. Typing in an excerpt edits the file itself (unsaved, as in its tab, which shows the change at once, and the other way around); ⌘S saves the files the tab changed, and one ⌘Z undoes one edit across every file it touched, in step with each file's own undo. Click a header or a line number, or ⌥↩, to open the file there; ⇧↩ or a header's ↕ shows three more lines around an excerpt. Find ▸ Find and Replace in Workspace adds a Replace field whose Replace All edits the results this way. A search over thousands of files opens a page of files at a time as the tab scrolls. | ⌥⇧⌘F, ⌥↩, ⇧↩ |
| | Comment selection, duplicate, move line, select next occurrence, soft wrap and font size. | ⌘/, ⌘D, ⌥⌘[ ⌥⌘], ⌥⌘E, ⌃⇧⌘L, ⌘+ ⌘- ⌃⌘0 |
| | Xcode-style `<#placeholder#>` snippets from completions, with Tab or ⌃/ jumping to the next placeholder. | Tab, ⌃/, ⌃⇧/ |
| **Settings** (from Zed) | zcode ▸ Settings… opens `~/.config/zcode/settings.json`: JSON with comments in Zed's format, applied the moment it's saved, in every window. `theme` (a name, or one for light and one for dark appearance), `buffer_font_family`, `buffer_font_size`, `tab_size`, `hard_tabs`, `soft_wrap`, `show_whitespaces`, `gutter.line_numbers`, `indent_guides`, `format_on_save` (through sourcekit-lsp, on ⌘S, Save All and `:w`), `remove_trailing_whitespace_on_save`, `ensure_final_newline_on_save`, `inlay_hints`, `vim_mode`, `git.inline_blame`, `git.show_authors`, `git.side_by_side_comparison`, `terminal.shell`, `agent_servers` and `agent` (the agent panel's agents, which one a conversation starts with, and `always_allow_tool_actions`), `edit_predictions` (predictive completion's provider, model, key and when it asks), `profiling.time_limit` (Profile stops recording after it), `lsp` (each language server's `enabled`, `binary`, `initialization_options` and `settings`), `auto_install_language_servers`, `formatter`, `distribution.notary_keychain_profile` (Distribute App's notarization credentials); zcode ▸ Open Default Settings lists them with their defaults (read-only). A project's `.zcode/settings.json` (zcode ▸ Open Project Settings File) wins over yours for its window, except the theme, font family, Vim mode, the agent settings, predictions, the notarization profile, a language server's `binary` and `auto_install_language_servers` (a cloned project can't name programs to run, or where your code and key are sent; it can only turn predictions off). The Editor menu's toggles (Authors, Inline Blame, Inlay Hints, Vim Mode, Wrap Lines, Invisibles, Side by Side / Inline Comparison) and ⌘+ ⌘- ⌃⌘0 write the file, keeping its comments, and show checkmarks. A mistake (a missing comma, `"tab_size": "four"`, an unknown setting) is shown with its line and column, and the settings it gave last stay until it's fixed. The toggles zcode kept in its session before move into the file once. | ⌘, |
| | zcode ▸ Key Bindings… opens `keymap.json`: blocks of bindings with an optional `context` (`Editor`, `Editor && vim_mode == normal`, `multibuffer`, `Terminal`, `Navigator && navigator == find`, `PlistEditor`, `Picker`, `Workspace`, …), over zcode's; an action by name (`zcode::Build` or `Build`, `[name, arguments]`), `null` to free a key, `["zed::Unbind", action]` to take a key off one action; multi-key sequences (`ctrl-x ctrl-s`). Bindings for a Vim mode come before Vim's own keys. Bindings that can't be made are reported; the rest apply. zcode ▸ Open Default Key Bindings lists zcode's. | — |
| | Themes are JSON files in `~/.config/zcode/themes`: gpui-component's format (the window's colors, the editor's and its syntax colors, a monospace font) or Zed's theme files. zcode ▸ Select Theme… lists them with Default Light and Default Dark. zcode ▸ Import Xcode Theme… converts one of Xcode's `.xccolortheme` themes (yours from `~/Library/Developer/Xcode/UserData/FontAndColorThemes`, or one Xcode ships) into a theme file and shows it: the editor's background, text, current line, selection, insertion point, invisibles, syntax colors with their bold and italic, and the font when it's installed. zcode ▸ Import Xcode Key Bindings… turns a set from `~/Library/Developer/Xcode/UserData/KeyBindings` into blocks of `keymap.json`: each command zcode has gets its shortcut, and loses zcode's default ones as in Xcode, and text editing bindings go to the editor; importing again replaces them. Xcode's files are only read. | — |
| **Vim mode** | Editor ▸ Vim Mode (off by default; `vim_mode` in the settings, for every window) in every editor tab: files in every pane, both sides of Code Review and multibuffers. Normal, insert, replace, visual, visual line and visual block modes, with the mode in the status bar (`-- INSERT --`, then any keys of a command not finished yet). Motions (`h j k l w W b B e E ge 0 ^ $ g_ \| gg G { } f F t T ; , % H M L + - _`, marks), counts, and the operators `d c y > < = g~ gu gU` over motions and text objects (`iw aw iW aW ip ap i" a" i' a' i( a( i[ a[ i{ a{ i< a< it at`, and the backtick quotes); `x X s S C D Y p P gp gP J gJ r R ~ o O i a I A gi gI ⌃A ⌃X`; registers (`"a`–`"z`, `"A`–`"Z` to append, `"0`–`"9`, `"-`, `"_`, `"+` `"*`, `".` `"/` `":`), the unnamed one being the system clipboard (as in Zed); `.` repeats the last change with what its insert typed (completions and closed brackets included); `u` and `⌃R`, an insert undoing in one step with the change that started it; `/ ? n N * #` with Vim's patterns and the matches highlighted; `m` with `'` and the backtick to go to marks; scrolling (`⌃D ⌃U ⌃F ⌃B ⌃E ⌃Y zz zt zb`) and folds (`zo zc za zR zM`); visual block `I`, `A` and `c` typing on every row at once. `:` commands: `:w`, `:wa`, `:q`, `:q!`, `:wq`, `:x`, `:qa`, `ZZ`, `ZQ` (`:q` closes the tab, as in Zed, and refuses unsaved changes without `!`; in a multibuffer `:w` saves the files the tab changed), `:s` with ranges (`%`, `.`, `$`, numbers, marks, `'<,'>`, `+N`) and the `g`, `i`, `I`, `n` flags, `:<line>`, `:d`, `:y`, `:j`, `:>`, `:<`, `:noh`, `:undo`, `:redo`, with ↑/↓ history. Every cursor of a multi-cursor editor moves and operates; a selection made with the mouse, ⌘D or a placeholder is visual mode. Escape closes completion, signature help and Find before it leaves insert mode. `gd` jumps to the definition, `gt` / `gT` switch tabs, `⌃O` / Tab go back and forward, `=` re-indents through sourcekit-lsp. zcode's own ⌃ shortcuts (⌃6, ⌃I, ⌃/, ⌃\`…) and every ⌘ key keep working; terminals, property list outlines and text fields aren't affected. | Editor ▸ Vim Mode |
| **Language intelligence** (sourcekit-lsp) | Completion, live diagnostics, Quick Help on hover, Jump to Definition (also ⌘-click), Find References and Rename across files. A rename opens a tab of the places it changed (a multibuffer, below), left unsaved: ⌘S saves every file, one ⌘Z undoes it everywhere. | ⌃Space, ⌃⌘J, ⌃⇧⌘F, F2 |
| | Edit All in Scope, Format File / Re-Indent, code actions and fix-its, Document Items outline, and workspace symbols in Open Quickly. | ⌃⌘E, ⌃⇧I / ⌃I, ⌃⌥⌘F, ⌃6 |
| | Semantic highlighting over tree-sitter's: what the compiler says each name is, so a type reads as a type at a call (`Square(side: 2)`), and properties, parameters and enum cases get their colors. Signature help while typing arguments: the call's signature above the cursor with the current argument in bold, from `(` until the call is closed (or Escape). Editor ▸ Inlay Hints (off by default, as in Zed): the type inferred for each binding without one (`let areas: [Double]`), and in C and Objective-C the parameter names before arguments (`add(left: 1, right: 2)`), drawn in the text without being part of it. | — |
| **Web apps** (Vite: HTML, CSS, JavaScript, TypeScript, Vue, Svelte, Astro) | The same language features for a web app inside the project, from the servers VS Code and Zed use: vtsls (JavaScript, TypeScript, JSX and TSX: completions with auto-imports, diagnostics, Quick Help, Jump to Definition, references, rename, code actions, inlay hints, semantic highlighting), the HTML, CSS (SCSS, Less) and JSON servers (JSON Schema Store's schemas for `package.json`, `tsconfig.json` and the other config files), Vue (Volar, with its TypeScript plugin in vtsls: zcode relays its `tsserver/request`s), Svelte and Astro, and, when the app uses them, ESLint (its fixes as code actions), Biome and Tailwind CSS (class completions, hover and color swatches). Each runs for the web app a file is in: the outermost folder with a `package.json`, `tsconfig.json` or `jsconfig.json` (an Xcode project's `web/` gets servers of its own next to sourcekit-lsp), started by the first file it serves. A file has every server for it: completions, code actions, colors and diagnostics are all of theirs; the first that answers gives Quick Help and definitions. The status bar shows the focused file's first server (installing, starting, indexing, failed) and how many more it has. Files in `node_modules` open read-only. | Editor ▸ Restart Language Servers |
| | Where the servers come from: `lsp.<id>.binary` in your settings, the web app's own `node_modules/.bin`, your login shell's `PATH`, or zcode installs them with npm into `~/Library/Application Support/zcode/language-servers` the first time one is needed (Node.js required; `auto_install_language_servers: false` turns that off). `lsp.<id>` also takes `enabled`, `initialization_options` and `settings` (merged over zcode's), as in Zed. Format File and `format_on_save` use the web app's Prettier when it has one (`formatter`: `auto`, `prettier` or `language_server`), Biome's formatter for a Biome app, the language server otherwise. | ⌃⇧I |
| | Emmet, built in (no Node): an abbreviation before the cursor is offered as a completion in HTML, the markup of Vue, Svelte and Astro files, JSX and TSX (`className`, `<img />`), and CSS, SCSS and Less (and a component's `<style>`): `ul.list>li*3`, `.card>h2{Title}+p`, `!`, `a:link`, `input:email`, `lorem10`, `m10-20`, `d:f`, `c#fc0`… Expanded, its first tab stop is selected and Tab visits the rest. Editor ▸ Emmet ▸ Expand Abbreviation expands without the menu; Wrap with Abbreviation… puts the selection (or the line) inside one, previewed as you type it (`ul>li*` makes an item of each line). Tags close as their `>` is typed in HTML, Vue, Svelte, Astro, JSX and TSX; ⌘/ comments markup as `<!-- -->` and CSS as `/* */` (a component's script and style blocks in their own language). | ⌃⌥E |
| | Find ▸ Find Call Hierarchy: the function at the cursor (in whichever pane is focused) as a tree in the Find navigator, with its callers (as in Xcode) or the calls it makes, each level expanded on demand; a caller opens at its call, the rest at their declarations, and a row's menu starts a new hierarchy from it. Find Ancestor Types, Find Descendent Types and Find Conforming Types show a type's supertypes or subtypes the same way. Editor ▸ Show Callers… lists the callers in a menu at the cursor. Features the language server doesn't offer are left out, and their commands say so. | ⌃⇧⌘H |
| **Xcode project indexing** | sourcekit-lsp learns how to compile an `.xcodeproj`'s files from zcode's own Build Server Protocol server, the job xcode-build-server does: each target's files come from the project (build phases, synchronized folders and their exceptions), their arguments from Xcode's own for indexing (`xcodebuild -showBuildSettingsForIndex`), the last index build's log, or the target's build settings. A file added to a target, a membership change or a build setting edited (in zcode, Xcode or an `.xcconfig` saved in zcode) reaches sourcekit-lsp within seconds, without a build, and a target the build never compiled still gets its arguments. zcode writes nothing into the project: sourcekit-lsp finds `zcode bsp` through a connection file in zcode's cache, and a project's own build server (`buildServer.json`, `.bsp/`) wins. A low-priority background "index build" writes the index (and a compilation database, which `"sourcekit_build_settings": "compilation_database"` hands sourcekit-lsp instead); saving source files refreshes it, so rename and references stay current. | — |
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
| **Build & run** | Scheme, configuration and run destination pickers. Destinations are grouped as in Xcode: My Mac, connected iOS, iPadOS, watchOS, tvOS and visionOS devices (from `devicectl`, with how they're connected or why they can't be used: not paired, Developer Mode disabled, unavailable), Any iOS Device (arm64) for building, then every available simulator, filtered by the scheme's `SUPPORTED_PLATFORMS`. Devices plugged in or out over USB appear and disappear at once; devices on the network when the destination menu opens, the window becomes active, or within a minute. | ⌃0, ⌃⇧0 |
| | Build, Run, Test, Stop and Clean. Files are saved first. Stop ends the app however it was started (on its own, under lldb-dap, which can leave it to launchd, or by a recording), and so do quitting zcode and closing the window. | ⌘B, ⌘R, ⌘U, ⌘., ⇧⌘K |
| | Profile: builds with the scheme's Profile action's configuration (Release unless the scheme says otherwise), then records the app under Instruments with `xctrace record --launch`, on the Mac, a simulator (booted and installed first) or a device (installed with `devicectl`), with the Run action's arguments and environment (or the Profile action's own). A template chooser lists `xctrace list templates` (Time Profiler, Allocations, Leaks, Animation Hitches, SwiftUI and App Launch first, then the rest and your own), unless the scheme's Profile action names its instrument; Profile… asks either way. The app's output streams into the console while it records. Stop ends the recording (xctrace saves the trace; a second Stop ends it outright), as does `profiling.time_limit` in `settings.json`; the trace opens in Instruments, and for Time Profiler the console lists the functions with the most samples, with their self and total share, from `xctrace export`. Profile Without Building and Build For ▸ Profiling too. Traces are kept in `~/Library/Caches/zcode/Traces` (the newest 20). | ⌘I, ⌥⌘I, ⌃⌘I, ⇧⌘I |
| | Archive: builds with the scheme's Archive action's configuration (Release unless the scheme says otherwise) for a generic device of the run destination's platform (Any iOS Device for an iOS simulator, any Mac for My Mac, or the first platform the scheme supports), signing with automatic provisioning as Xcode does, into Xcode's own Archives folder (`~/Library/Developer/Xcode/Archives/<date>/<Scheme> <date>, <time>.xcarchive`), so Xcode's Organizer lists zcode's archives and the other way around. The output goes to the build log and the Reports navigator; a signing error says to choose the team in the target's Signing & Capabilities tab. Then the Organizer opens on the new archive (unless the scheme's Reveal Archive in Organizer is off). | Product ▸ Archive |
| | Window ▸ Organizer: the Archives folder as a tab, archives grouped by app (iOS, macOS… and Other Items) with their creation date, version, description, identifier, team, signing identity and architectures; Show in Finder, Delete Archive (to the Trash), Refresh. Distribute App offers Xcode's methods: App Store Connect and TestFlight Internal Only (uploaded to App Store Connect, or exported), Release Testing, Enterprise and Debugging for device apps, Developer ID and Copy App for Mac apps, with their options (symbols, version and build number management, stripping Swift symbols) and the folder to export to. zcode writes the `ExportOptions.plist` and runs `xcodebuild -exportArchive -allowProvisioningUpdates` (`destination: upload` to upload). Developer ID apps can be notarized: zipped, submitted with `notarytool submit --wait` and the keychain profile you stored once with `xcrun notarytool store-credentials` (remembered as `distribution.notary_keychain_profile`), and the ticket stapled; a rejection lists Apple's reasons in the Issue navigator. | ⌥⇧⌘O |
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
| **Monitoring** | Memory is sampled every 2 s for the life of the app. The status bar shows zcode's footprint and the language servers' together (including sourcekit-lsp's `SourceKitService` XPC helpers and the processes the web's servers start, such as vtsls's tsserver), and turns orange above 1.5 GB idle for zcode or 4 GB for the language server. View ▸ Performance shows startup timings, the current and 30 s idle-baseline footprint, peak, and a history graph. `--perf-log <file>` (or `ZCODE_PERF_LOG`) appends every sample as a JSON line for long-term tracking. `zcode --startup-bench` prints startup and memory as JSON and quits. `scripts/bench.py` runs the scale benchmarks on firefox-ios, against a build of `master`, with gates (`docs/scale.md`). | — |
| **The app** | `scripts/package.py` builds `zcode.app` (icon, `Info.plist` with the version from `Cargo.toml`, the document types for `.xcodeproj`, `.xcworkspace`, Swift sources, source code and folders as an alternate handler so Xcode stays the default), signs it (Developer ID with the hardened runtime, or ad hoc for local use), notarizes and staples it (`notarytool` with a keychain profile; nothing in the repo), and makes the zip, the dmg and the update feed. Finder opens projects in the running zcode or starts it. zcode ▸ Install CLI links `zcode` into `/usr/local/bin` (an administrator prompt when needed, as Zed's): `zcode <path>` hands the path to the running zcode in ~50 ms or starts the app through LaunchServices and returns. zcode ▸ Open at Login registers the bundle's LaunchAgent (`zcode --background`) with SMAppService, listed under zcode in System Settings ▸ Login Items. zcode ▸ Check for Updates… (and a daily check, `auto_update` in settings.json) reads a JSON feed, downloads the release, checks its SHA-256 and signature (the running app's team), swaps it into the bundle's place and offers Restart Now. | — |

The full Xcode 27 menu and shortcut inventory this was matched against is in
[`docs/xcode-inventory.md`](docs/xcode-inventory.md), and remaining gaps are in
[`docs/parity.md`](docs/parity.md).

## zcode as an MCP server

`zcode mcp` speaks the [Model Context Protocol](https://modelcontextprotocol.io)
on stdin and stdout. It connects to the running zcode over its Unix socket
(the one `zcode <path>` hands paths to) and relays messages; the protocol is
answered inside zcode, where the project is. zcode has to be running (a
window, or `zcode --background`); without it `zcode mcp` says so and exits.

Add it to Claude Code (the `zcode` binary on your `PATH`, or its full path):

```
claude mcp add zcode -- zcode mcp                  # this project only
claude mcp add --scope user zcode -- zcode mcp     # every project
```

Other clients take the same command and argument:

* Codex: `codex mcp add zcode -- zcode mcp`, or in `~/.codex/config.toml`
  `[mcp_servers.zcode]` with `command = "zcode"` and `args = ["mcp"]`.
* Gemini CLI: `gemini mcp add zcode zcode mcp`, or `"mcpServers": { "zcode":
  { "command": "zcode", "args": ["mcp"] } }` in `~/.gemini/settings.json`.
* Cursor: the same `mcpServers` entry in `~/.cursor/mcp.json` (or a
  project's `.cursor/mcp.json`).
* Zed: `"context_servers": { "zcode": { "command": "zcode", "args": ["mcp"] } }`
  in its `settings.json`.
* Anything else: run `zcode mcp` as a stdio server.

zcode's own agent panel needs none of this: every agent it starts gets the
server in its session (`session/new`'s `mcpServers`), unless
`"agent": { "zcode_mcp_server": false }` in `settings.json`.

Which project: the one the client started `zcode mcp` in (its working
folder inside a project zcode has open), else the frontmost window; every
tool also takes `project` (a path in the project). The agent panel's agent
always works on its own window's project.

| Tool | What it does |
|---|---|
| `XcodeListSchemes`, `XcodeSwitchScheme` | The schemes, configurations and test plans, and which are chosen; choose a scheme as the toolbar does |
| `XcodeListRunDestinations`, `XcodeSwitchRunDestination` | My Mac, simulators (booted or not) and devices for the scheme; choose one |
| `BuildProject` | Product ▸ Build: waits, then success, the errors and warnings (file, line, column) and the log's file |
| `RunProject` | Product ▸ Run (under the debugger when breakpoints are set): returns once it's running, with its first console output |
| `StopProject` | Product ▸ Stop: the build, the app and the debugger |
| `RunAllTests`, `RunSomeTests` | Product ▸ Test, or some tests by the Test navigator's names: every test's result and duration, the failures at their lines |
| `GetTestList` | The tests found in source, with where they are and their latest results |
| `XcodeListNavigatorIssues` | The Issue navigator: live sourcekit-lsp diagnostics and the last build's issues, for a file or all |
| `GetBuildLog` | The build log, condensed or raw, filtered; the last builds' reports and log files |
| `GetConsoleOutput` | The app's and the debugger's console |
| `XcodeGetCurrentFile` | The focused editor's file, cursor, selection and selected text, unsaved changes, and its text if asked |
| `OpenFile` | Opens a file at a line and column, so you see it |
| `EvaluateInDebugger` | An LLDB command (`po`, `p`, `v`, `bt`, `expr`) in the paused frame, shown in the console |
| `TakeScreenshot` | A PNG of the project window (a pixel per point) or of a simulator's screen (`simctl io screenshot`); never a device |
| `ProfileProject` | Product ▸ Profile for a time limit (10 s unless told; Time Profiler unless another template is named): the trace, opened in Instruments, and the heaviest functions |
| `ArchiveProject` | Product ▸ Archive: the archive's path, app, version and team, or the errors. It never exports or uploads; Distribute App in the Organizer does that |

Xcode 27 has an MCP server too (`xcrun mcpbridge`, for a running Xcode);
where it has the tool, zcode's has its name, so instructions written for
Xcode's tools find zcode's. The arguments are zcode's own.

The protocol: zcode answers the handshake revisions (`initialize` in
2025-11-25, 2025-06-18 or 2025-03-26) and the current one, 2026-07-28 (the
revision in every request's `_meta`, `server/discover`, `resultType`;
an unknown revision is refused with the versions zcode speaks). Tool
results carry `structuredContent` and the same JSON as text; screenshots
are `image` content. A request with a `progressToken` gets
`notifications/progress` while a build or test runs (the step it's on,
tests run and failed); `notifications/cancelled` stops the build, run or
test (and the request gets no answer, as the spec says); a client that
disconnects stops what it started.

Security: the socket is in zcode's own cache folder (mode 0700), created
readable and writable by its owner alone (0600), and zcode checks every
connection's peer (`getpeereid`) and refuses other users before reading
anything; `zcode mcp` checks that the socket's other end is the same user
too. The control channel (`ZCODE_DEBUG_DIR`) is a separate test hook and
isn't reachable through MCP. Tools that start, stop or evaluate something
(`BuildProject`, `RunProject`, `StopProject`, `RunAllTests`,
`RunSomeTests`, `EvaluateInDebugger`, `ProfileProject`, `ArchiveProject`) called by the agent panel's agent
ask in the panel's thread first, with Allow, Always Allow (for the
conversation) and Reject, unless `agent.always_allow_tool_actions` is on;
the panel knows its agent by a random token in the server's environment.
Other clients ask their users themselves, as Claude Code does before
running an MCP tool.

`scripts/mcp_client.py` is a small client for trying it:
`python3 scripts/mcp_client.py tools`, `python3 scripts/mcp_client.py call
BuildProject`, `--modern` for 2026-07-28.

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
  5.3 ms on this branch, 5.4 ms on `master`). Devices are listed again when
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
* The agent panel costs nothing until it's opened: no ACP code runs at
  startup, and a session that had it open shows it after the first frame
  without starting an agent; the agent starts (its command found on the
  login shell's `PATH`, read off the main thread once) when the first
  message is sent. Its process and the reading of what it sends are on
  threads of their own; each update re-lays out only the rows it changed
  in a virtual list. Cold launch, alternating runs against `master`: the
  welcome window's first pixels at 242.5 ms against 249.9 ms (median of 9).
  The binary grows by 1.9 MB (the protocol's types).
* The MCP server costs nothing until a client connects: the socket was
  already there for `zcode <path>`, and a connection's reading and writing
  are on threads of their own. `zcode mcp` itself is a relay that checks
  its first argument before anything else starts, so it starts in a few
  milliseconds. Cold launch, alternating with `master` (load average
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
  15 against `master` (load average 7–10): the welcome window's first
  pixels at 263.8, 257.9 and 260.7 ms against 258.4, 259.8 and 263.8 ms.
  The binary grows by 1.4 MB (`ureq` and `native-tls`).
* Profile costs nothing until it's used: Instruments' templates are listed
  (`xctrace list templates`, on a thread of its own) the first time
  Profile asks for one, a recording's output streams like a run's, and the
  summary's export is read off the main thread. Cold launch, three
  alternating rounds of 15 against `master` (load average 10–25): the
  welcome window's first pixels at 236.6, 234.9 and 237.7 ms against
  231.8, 240.4 and 235.3 ms.
* Archive and the Organizer cost nothing until used: the Organizer reads
  the Archives folder on a thread of its own when it opens (and after
  zcode archives, exports or deletes), and Distribute App's export,
  notarization and stapling stream like a build. Cold launch, three
  alternating rounds of 12 against `master` (load average ~8): the
  welcome window's first pixels at 237.1, 228.5 and 237.2 ms against
  230.5, 234.6 and 230.1 ms.
* Asset catalogs cost nothing until one is opened: the catalog is read on
  a thread of its own (3,000 image sets in ~2.5 s in the e2e check, the
  window answering throughout), and thumbnails are decoded in the
  background at most four at a time, only for the wells and tiles on
  screen (the outline and a folder's tiles are virtual lists), scaled to
  192 pixels and kept in a cache of at most 64 MB that drops the least
  recently shown (their textures freed too). Edits run in the background.
  Cold launch, three alternating rounds of 10 against `master` (load
  average 10–14): the welcome window's first pixels at 231.3, 224.7 and
  219.2 ms against 227.4, 229.2 and 223.7 ms. The binary grows by 1.5 MB
  (the PDF and image decoding zcode now calls; the crates were already
  linked).
* SwiftUI previews cost nothing at startup: a Swift file is searched for
  previews when it opens and 300 ms after typing stops, in the background
  (files that don't mention `#Preview` or `PreviewProvider` aren't
  parsed), and the gutter reads what was found. Show in Xcode Canvas waits
  for Xcode on a thread of its own. Cold launch, three alternating rounds
  of 10 against `master` (load average 12–16): the welcome window's first
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
  against a build of `master`'s commit (load average 20–50): the welcome
  window's first pixels at 251.1, 260.1 and 259.4 ms against 269.9, 269.0
  and 261.8 ms. The binary grows by 0.05 MB (`objc2-service-management`).
* Settings cost nothing measurable at startup: the user's settings,
  keymap and the theme they name are read and parsed on a thread of their
  own started at the top of `main()`, and the first window takes what it
  read (~0.06 ms on the main thread); with no files it's one failed
  `open` each. A project's `.zcode/settings.json` is read when its window
  opens, and every file is read again when FSEvents says it changed
  (see "Idle" below). Cold launch, alternating runs against `master` (load average
  9–17): the welcome window's first pixels at 253.2 ms with no settings
  files and 255.0 ms with settings, a custom theme, a font and key
  bindings, against 259.0 ms on `master` (median of 20); ZKit at 262.4 /
  265.2 ms against 262.7 ms (median of 12).
* Swift highlighting uses a compact tree-sitter query
  (`src/languages/swift-highlights.scm`) that compiles in ~11 ms. The upstream
  query takes ~185 ms (`cargo run --release --example query_bench`). A
  language's queries are compiled once and shared by every editor (a patch
  to gpui-component's highlighter; upstream compiles them for each editor,
  and an injected language's on every parse): opening 30 firefox-ios Swift
  files costs 1.0 s of CPU against 1.4 s on `master`. When a project opens,
  its restored tabs' languages are compiled on a background thread while
  the window is created, so the shown tab's highlighter no longer compiles
  them on the main thread: ZKit with four Swift tabs restored shows its
  first frame at 201 ms against 218 ms on `master` (median of 15,
  alternating, load average 9–20); the welcome window is unchanged
  (232.5 ms against 233.9 ms to its first pixels, median of 15).
* Big files: an edit costs what it touches. The gutter finds a row's fold
  by binary search, not a scan of every fold (Page Down ×5 in a 15 MB JSON
  file: 15–19 ms of work against 23–31 ms on `master`); an edit's fold
  candidates come from the syntax tree's nodes around it, not from every
  node at the top level; and a language server is sent the editor's own
  edits (`EditJournal`, recorded as each edit is applied), checked against
  the text, with a rope comparison that skips the chunks both texts share
  when they don't add up. Typing near the end of a 50,000-line Swift file:
  1.9–2.9 ms of work per keystroke against 6.7–7.8 ms on `master`
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
  against `master`, load average 3–5): typing 3.2 ms of work a keystroke
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
  each redraw and so redrew the window). Over 60 s idle, against `master`
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
  line before). Heap added by opening a file, against `master`: a 15 MB
  JSON file 22 MB against 514 MB, a 3.3 MB minified script 5 MB against
  194 MB, a 1.2 MB Swift file (which keeps its tree) 55 MB against 61 MB.
  Files up to 64 MB open (16 MB before; a 60 MB log adds 88 MB and types at
  3 ms of work a key); past 16 MB a file goes to no language server and
  gets no change bars.
* **Tabs:** an editor in a tab no pane shows lets go of its layout, and a
  layout's lines share their whitespace marks instead of each holding a
  copy (a line's layout was 9.1 KB). 40 firefox-ios Swift tabs add 15 MB of
  heap against 32 MB on `master`, about half of it their syntax trees.
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
                    views, Settings… and the theme and import pickers; the agent panel (agent/: the
                    conversation, the agent's file reads and writes through buffers, @-mentions);
                    predictive completion (predictions.rs: when to ask, cancelling, the status bar);
                    Profile's template chooser (profile.rs); Archive, the Organizer tab and Distribute App's
                    sheet (archive.rs, organizer.rs, distribute_sheet.rs); the asset catalog editor as a tab
                    (asset_catalog.rs: outline, wells, inspector, edits and undo; asset_catalogs.rs: opening it);
                    SwiftUI previews' gutter marks and Show in Xcode Canvas (previews.rs)
  bsp/              the Build Server Protocol server sourcekit-lsp starts for an Xcode project (`zcode bsp`,
                    relayed to the running zcode's socket; its connection file found through XDG_DATA_HOME):
                    the build graph (graph.rs: targets, files, arguments from Xcode's index settings, the
                    index build's log or build settings), reading the project and asking xcodebuild
                    (inputs.rs), connections answered on threads of their own (server.rs), the graph kept
                    current as the project, the database, the scheme and destination change (model.rs)
  lsp/              JSON-RPC client; the store of a project's servers (one per kind and folder) keeping
                    documents in sync with ranged changes (sync.rs: the editor's own edits, or where the
                    texts differ), pushed and pulled diagnostics; which server
                    serves which files and folders, and what each is told (servers.rs); finding and
                    installing their programs (install.rs); editor provider adapters merging several
                    servers' answers; Prettier; inlay hints, semantic tokens and call/type hierarchies
                    converted for the editor
  languages/        grammar registrations (Swift's query, JSX's tags, TSX, Vue's blocks), editing rules
                    for markup and CSS (tags closed as typed), Emmet in the editor (emmet.rs)
  build/            xcodebuild/swift/simctl/devicectl queries, USB devices attaching and detaching (usb.rs,
                    IOKit's notifications), streaming runner (programs started with
                    no signals blocked, ended by Stop wherever they ended up), log/issue/test parsing,
                    compilation-database extraction; profile/: Product ▸ Profile (xctrace's templates and
                    recording, opening Instruments, a Time Profiler export's heaviest functions); archive/:
                    Product ▸ Archive and Distribute App (archives in Xcode's folder read from their
                    Info.plist, ExportOptions.plist and xcodebuild -exportArchive, notarytool and stapler)
  packages/         Swift package dependencies: rules and versions, Package.resolved, checkouts,
                    Package.swift edits, git queries, resolve/update/reset
  testing/          tests found in source, test plans, test selections per runner, results → tests
  previews.rs       SwiftUI previews found in source (#Preview, PreviewProvider) with testing/'s parser
  xcode_app.rs      handing work to Xcode: Open in Xcode, Show in Xcode Canvas (xed, then System Events
                    for Editor ▸ Canvas); ZCODE_XCODE_COMMANDS records the commands instead
  scheme/           Edit Scheme options: .xcscheme read and edited in place, what launches get
  xcassets/         asset catalogs: sets and folders read from disk, slots laid out as Xcode's editor shows
                    them (slots.rs), colors and their input methods (color.rs), Contents.json written as
                    Xcode writes it (json.rs), edits as file operations that undo themselves (edit.rs),
                    thumbnails decoded off the main thread and cached (thumbnails.rs)
  plist/            property lists: XML with byte ranges, binary, in-place edits, Xcode's key definitions
                    (generated by scripts/plist_definitions.py)
  signing/          teams, identities and provisioning profiles; capabilities and where they live
  acp/              the Agent Client Protocol's client side: JSON-RPC to an agent on stdio (its requests
                    to zcode included), the agents and how to start them, a conversation built from
                    session updates
  predictions/      predictive completion's providers: the Claude API streamed over HTTPS on a thread of
                    its own (claude.rs, sse.rs), a language server's inlineCompletion (lsp.rs); the
                    file around the cursor, the latest edits, never-sent files
  mcp/              zcode as an MCP server: `zcode mcp`'s relay (bridge.rs), a connection served on the
                    main thread with cancellation and progress (server.rs), both protocol eras
                    (protocol.rs), the tools (tools.rs; workspace/mcp.rs is their side of the window),
                    the agent panel's tokens
  instance.rs       the Unix socket: `zcode <path>` handed to the running zcode, `zcode mcp`'s and
                    `zcode bsp`'s connections; the same user only
  bundle.rs         the zcode.app this binary runs from (path, version from its Info.plist), a launch from the
                    shell's `zcode` link handed to LaunchServices (`open -a`), Finder's file URLs
  cli.rs            zcode ▸ Install CLI: the link in /usr/local/bin, an administrator prompt when needed
  login_item.rs     zcode ▸ Open at Login: the bundle's LaunchAgent (`--background`) through SMAppService
  updates.rs        zcode ▸ Check for Updates… and the daily check: the JSON feed, download, SHA-256,
                    codesign, the swap into the bundle's place, Restart Now
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
                    read and edited in place, read off the main thread at launch and again when they change;
                    the theme in effect; Xcode's .xccolortheme themes and .idekeybindings key bindings
                    converted (plist/ reads XML, binary and old-style property lists)
  fs_watch.rs       files changed on disk through FSEvents (one stream, on a thread of its own): open
                    buffers, settings, the git directory and the build server's projects are looked at
                    when something under them changes, polled only where the stream can't cover them
  perf.rs           startup timing and memory sampling (proc_pid_rusage); how long the scan, the tree, a
                    search and Open Quickly's matching took, for the scale benchmarks
  debug.rs          scriptable UI control channel for tests (ZCODE_DEBUG_DIR); `bench …` times inputs to the
                    frame they cause and reports what scripts/bench.py polls
  screenshot.rs     zcode's own windows and simulators as PNGs (the control channel, startup bench, MCP)
crates/
  zcode-editor/     the code editor: rope buffer, display map (soft wrap, its index runs of the lines
                    that don't wrap; folding; lines over 4 KB measured per character and laid out only
                    where visible: long_line.rs, base/long_lines.rs), highlighting (no syntax tree for
                    large or minified files: base/large_file.rs),
                    undo, multiple cursors, edit events and a journal of edits as they're applied,
                    linked views of one buffer, multibuffers (excerpts of many
                    buffers, one undo step across them), Vim mode (a keystroke interceptor with
                    motions, text objects, operators, registers, search and `:` commands), edit
                    predictions as ghost text on inlays and blocks, LSP popovers, find panel
                    (docs/editor-engine.md)
  zcode-emmet/      Emmet, std only: HTML, JSX and CSS abbreviations to LSP snippets, the abbreviation
                    before the cursor, Wrap with Abbreviation
vendor/
  gpui-component/   gpui-component 0.7.3 plus small patches (ZCODE-PATCH.md)
  gpui-pre-macos/   GPUI's macOS backend 0.3.9, its display link paused while a window is idle (ZCODE-PATCH.md)
scripts/
  e2e.py            end-to-end UI tests
  bench.py          scale benchmarks on firefox-ios (fetched into ~/Library/Caches/zcode-bench), gated against
                    a build of master made there (docs/scale.md)
  fake_acp_agent.py a scripted ACP agent for the agent panel's tests (no network or account); it also
                    calls the MCP servers it's handed
  mcp_client.py     a stdio MCP client for the MCP server's tests and for trying it
  fake_anthropic.py the Claude API's Messages endpoint, streaming scripted answers, for the predictions'
                    tests (no key or network)
  lsp_proxy.py      the real sourcekit-lsp behind a proxy that logs requests, hides capabilities or
                    answers inlineCompletion itself
  fake_xctrace.py   xctrace for Profile's tests: templates, a recording until SIGINT, a time limit or
                    the app's exit, and a Time Profiler export (no Instruments, no Developer Tools access)
  fake_distribute.py  xcodebuild -exportArchive, notarytool and stapler for Distribute App's tests: exports
                    copy the app, uploads and notarization are pretended (nothing reaches Apple)
  appkit_floor.swift  minimal native AppKit app, the cold-launch reference
  package.py        zcode as a Mac app: bundle (Info.plist, icon, the login item's agent), sign, notarize,
                    zip, dmg, appcast, release; bundle/ holds the icon's drawing (make_icon.swift), the
                    entitlements and the agent's plist
  fake_update_feed.py  a release feed on a local port for the update check's tests (nothing reaches a server)
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
tree-sitter highlighter) is vendored with a startup patch, a visibility
patch for the highlighter and one that compiles each language's queries
once, all described in `vendor/gpui-component/ZCODE-PATCH.md`. GPUI's
macOS backend (`gpui-pre-macos`, Zed's `gpui_macos`, Apache-2.0) is
vendored with one patch that pauses an idle window's display link through
the frame waker GPUI's core already has
(`vendor/gpui-pre-macos/ZCODE-PATCH.md`). Files changed on disk come from
FSEvents through `notify` (CC0-1.0, already built for gpui-component).
Opening large files without a syntax tree follows VS Code's and JetBrains'
documented behavior (limits past which they turn features off); the
limits and the code are zcode's.
USB devices attaching are noticed through IOKit's notifications, Apple's
framework called directly (no new dependency).
Long lines are measured from each character's width and laid out in pieces
by code of zcode's own (the soft wrap of a long line uses
`unicode-linebreak`, already a dependency); the idea of shaping only what's
visible, and of plain text past a length, is VS Code's documented behavior,
none of its code used.
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
The agent panel speaks the Agent Client Protocol (agentclientprotocol.com,
Apache-2.0) with the message types of its `agent-client-protocol-schema`
crate (Apache-2.0), which brings `serde_with` (MIT OR Apache-2.0),
`schemars`, `derive_more` and `strum` (MIT); the transport, the
conversation, the panel and what agents do to buffers are zcode's own. The
panel's behavior (streaming, tool calls with diffs, permission prompts,
Follow, @-mentions, Review / Reject / Keep) follows Zed's public
documentation of its agent panel (Zed's code is GPL; none of it was read);
⌘0 and File ▸ New ▸ Conversation (⌃⌘H) are Xcode 27's, whose Coding
Assistant also runs Claude Code, Codex and ACP agents.
The MCP server is written from the Model Context Protocol's specification
(modelcontextprotocol.io; 2025-11-25 and 2026-07-28) without an SDK: its
transport, sessions and tools are zcode's own. Its tool names are those of
Xcode 27's MCP server (`xcrun mcpbridge`), read from the names Xcode's
coding assistant lists; their arguments and results are zcode's.
Predictive completion's name, menu item and ⌃⌥Space are Xcode 27's (Show
Predictive Completion); showing a prediction as gray text that typing
through keeps, the `edit_prediction` key context, `edit_predictions` in
the settings and `disabled_globs` follow Zed's public documentation (Zed's
code is GPL; none of it was read), and the provider is written from the
Claude API's public documentation of the Messages API and its streaming
events, without an SDK. HTTPS goes through `ureq` (MIT OR Apache-2.0) with
`native-tls` (MIT OR Apache-2.0), which uses macOS's Security framework
and trust store; `globset` (MIT OR Unlicense, already there through
`ignore`) matches the never-sent files.
Profile runs Instruments' own command line, `xctrace` (from the active
Xcode), and opens its traces in Instruments; its commands, keys
(⌘I, ⌥⌘I, ⌃⌘I, ⇧⌘I) and the Profile action read from the scheme are
Xcode's. The summary reads `xctrace export`'s XML with `roxmltree` (MIT
OR Apache-2.0, already built for gpui's SVGs).
Archive and Distribute App run Xcode's command line tools (`xcodebuild
archive` and `-exportArchive`, `notarytool`, `stapler`, `ditto`); the
archive layout, the Archives folder, the method names, Window ▸ Organizer
and ⌥⇧⌘O are Xcode's, and the `ExportOptions.plist` keys are those
`xcodebuild -help` documents. No new dependencies.
The asset catalog editor follows Xcode's (the outline, wells in rows by
device and appearance, the attributes inspector, Xcode's names for new
sets); `Contents.json` is read with `serde_json` and written in the format
`NSJSONSerialization` gives Xcode (pretty-printed, sorted keys), checked
against Foundation itself. Thumbnails decode with `image` (MIT OR
Apache-2.0) and `resvg` (Apache-2.0 OR MIT), both already built for GPUI,
and PDFs with macOS's Core Graphics through `objc2-core-graphics` (Zlib OR
Apache-2.0 OR MIT, already built for AppKit); other formats through
macOS's `sips`.
Show in Xcode Canvas runs Xcode's own `xed` and drives Xcode's Editor ▸
Canvas and Refresh Canvas through System Events (AppleScript, `osascript`);
the canvas's key (⌥⌘↩) and menu names are Xcode 27's, read from its
command definitions. Previews are found with `tree-sitter-swift` (MIT),
already there for highlighting and tests. No new dependencies.
The app bundle is made by `scripts/package.py` with macOS's own tools
(`iconutil`, `codesign`, `notarytool`, `stapler`, `ditto`, `hdiutil`); the
icon is drawn by `scripts/bundle/make_icon.swift`. Install CLI's link in
`/usr/local/bin` with an administrator prompt, Open at Login and a daily
update check follow Zed's public behavior (Zed's code is GPL; none of it
was read), written from scratch; the login item uses macOS's
ServiceManagement framework through `objc2-service-management` (Zlib OR
Apache-2.0 OR MIT, like `objc2`), and the updater uses `ureq` (already
there), CommonCrypto's SHA-256 (part of libSystem) and `codesign`. Sparkle
(MIT) was considered and not used: it would add an Objective-C framework,
its XPC services and EdDSA keys to the bundle for what those do here.
The web's language servers aren't part of zcode: they're npm packages
(vtsls, `vscode-langservers-extracted` for HTML, CSS, JSON and ESLint,
`@vue/language-server`, `svelte-language-server`,
`@astrojs/language-server`, `@tailwindcss/language-server`: MIT) a web
app has, you install, or zcode installs from npm when one is first
needed; Biome and Prettier are only ever the web app's own. Which servers
a file gets, what they're told and how Vue's TypeScript requests reach
vtsls follow those projects' documentation and Zed's public
documentation of its web language support (none of Zed's code was read).
The web's grammars are tree-sitter's (MIT), through gpui-component; the
JSX and Vue queries are zcode's. Emmet is written from Emmet's
documentation of its abbreviation syntax and snippets (`crates/zcode-emmet`,
no Emmet code); Expand Abbreviation, Wrap with Abbreviation and ⌃⌥E are
zcode's choices (Xcode has no Emmet).
The build server is written from the Build Server Protocol's specification
(build-server-protocol.github.io) and sourcekit-lsp's documentation of its
extensions and of how it finds a build server (Apache-2.0 with the Runtime
Library Exception; its source was read to check the installed version's
method names and search order, none of it copied). It asks Xcode's own
`xcodebuild -showBuildSettingsForIndex` and `-showBuildSettings`.
xcode-build-server (MIT) does the same job from build logs; none of its
code is used. No new dependencies.
The scale benchmarks (`scripts/bench.py`) run zcode on firefox-ios
(Mozilla Public License 2.0), fetched at a pinned commit into a cache
outside the repository and only read; nothing of it is copied into zcode.
zcode doesn't use any GPL code from Zed.

## Testing

```
cargo test                          # unit tests (zcode and crates/zcode-editor)
cargo test -- --ignored             # needs Xcode: sourcekit-lsp (also through `zcode bsp`), lldb-dap, and xcodebuild on an edited project
                                    # (network_* tests need network access)
cargo build --release && python3 scripts/e2e.py [--simulator] [--network] [--compare-apps]   # drives the real UI
python3 scripts/bench.py [--rounds 3] [--skip-index]   # scale benchmarks on firefox-ios, against master (docs/scale.md)
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
* the build server on a copy of HelloZ: sourcekit-lsp connecting to
  `zcode bsp` through zcode's connection file (nothing written into the
  project) and asking for Xcode's index arguments; a file added to the
  synchronized folder working before any build (⌃⌘J into another file); a
  build setting edited in `project.pbxproj` reaching the open file (an
  `#if` block's error appears); a membership exception taking a file out
  of its target; `"sourcekit_build_settings": "compilation_database"`
  restarting sourcekit-lsp on the database, and back
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
  scrolling cost, what the server was sent rebuilding the editor's text
  (typing, and two cursors 21 lines apart sent as two one-letter changes),
  and a server with these capabilities hidden (nothing
  asked for, ⌃⇧⌘H says so, completion still works)
* web languages on a copy of `samples/HelloWeb` (a Swift package with a
  Vite app in `web/`), against the real servers, installed by zcode with
  npm into a folder runs share (`ZCODE_LANGUAGE_SERVERS_DIR`): vtsls
  rooted at `web/` next to sourcekit-lsp for the Swift; TypeScript
  completions, live diagnostics, an auto-import and Jump to Definition
  into another file; the HTML server's completions and a tag closed as
  its `>` is typed; Emmet in HTML (a completion, Tab through its stops,
  ⌃⌥E) and CSS (`m10-20`); ⌘/ as `/* */` and `<!-- -->`; the JSON
  server's diagnostics; a Vue template's expression completed by vtsls
  through Vue's server; Format File through the app's Prettier (a
  stand-in in `node_modules/.bin`); `node_modules` read-only;
  `lsp.<id>.enabled` and Restart Language Servers. With `--network`, ESLint's
  pulled diagnostics and Tailwind's classes from the app's own packages
* predictive completion on a copy of ZKit: off by default (nothing asked
  or shown); the Claude API provider against `scripts/fake_anthropic.py`
  (a local server streaming the Messages API's events, so no key or
  network): one request for a burst of typing, carrying the file around
  the cursor and the latest edits, streamed in pieces as an inlay (and a
  block for more lines), Tab as one ⌘Z, typing through it, ⌥] and Escape,
  a caret moved during a slow request cancelling it and closing the
  connection, Vim's normal mode, a read-only checkout and a `.env` left
  alone, an overloaded API, a broken stream and a refused key in the
  status bar, ⌃⌥Space with automatic predictions off, typing latency with
  predictions on and off, and the key in no log; then the language server
  provider through `scripts/lsp_proxy.py` answering `inlineCompletion`
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
* the agent panel on a copy of ZKit, with `scripts/fake_acp_agent.py` as
  the agent (it streams, asks permission, and reads and writes files
  through zcode, as its prompts direct): nothing loaded at startup and no
  agent until a message is sent; ⌘0; an answer streamed; what zcode tells
  the agent it can do; a permission prompt, then a write into the open
  buffer (the agent having read its unsaved edit), saved, undone by one
  ⌘Z; Reject; a created file and a closed file the agent changed on disk
  itself in buffers ⌘Z undoes; Review against the file before the agent,
  Reject putting it back or trashing a created file; a write outside the
  project refused; `@` opening the mention picker and mentions sent with
  the buffer's text (and a range's lines); Follow keeping the keyboard in
  the panel; the plan, modes, Stop (a waiting permission request
  cancelled); a crash explained and the next message starting another
  agent; signing in, then the waiting message; New Conversation letting go
  of the conversation's buffers; the agent ended with zcode and the panel
  and Follow back after a relaunch, without an agent; zcode's MCP server
  in the session it starts
* zcode as an MCP server on a copy of ZKit, through `zcode mcp` and
  `scripts/mcp_client.py`: no zcode running; the socket private (0600) and
  not held up by a client that says nothing; `initialize` in two revisions,
  2026-07-28 with `server/discover` and an unknown revision refused; the
  tools and their hints; schemes and destinations listed and switched; a
  test run cancelled while it builds (stopped, no answer); a build with an
  error (progress, the error's file and line, the Issue navigator, the
  raw log), then fixed; every test's result, one test, a failing test at
  its line, the test list; run and the console; Open File and the current
  file's selection and unsaved edit; LLDB at a breakpoint; a window
  screenshot; the agent panel's (fake) agent calling the tools, asked
  before building and stopping (Allow, Reject, Always Allow for the
  conversation); `ProfileProject` with a time limit (the trace and its
  heaviest functions); with `--simulator`, a screenshot of the simulator the
  sample runs in
* Profile on a copy of HelloClassic through `scripts/fake_xctrace.py`
  (recording needs Developer Tools access, and a real one opens
  Instruments on your screen; `ZCODE_OPEN_TRACE_LOG` notes the trace
  instead): the template chooser, a Release build, xctrace's arguments
  (the template, the scheme's arguments and environment, the app's output
  in the console), Stop saving, opening and summarizing the trace, and the
  app the recording launched gone; Profile Without Building with
  `profiling.time_limit`; the scheme's instrument skipping the chooser and
  Profile… asking anyway; Build For ▸ Profiling. On the fake device
  (installed, recorded there) and, with `--simulator`, on a simulator
* the asset catalog editor on a copy of HelloClassic given the sets of
  `tests/fixtures/xcassets` (Contents.json files in Xcode's format) and a
  copy of HelloZ: the tab, outline, rows and wells, PNG, PDF and SVG
  thumbnails, app icon wells and a data set; a color component typed into
  its field and undone to Xcode's bytes; files dropped through the window's
  own file-drop events (as Finder's drags arrive) into wells and onto the
  outline; New Image Set and Color Set byte for byte, a rename typed,
  Delete, every edit undone byte for byte with `project.pbxproj`
  untouched; the tab after a relaunch; in the synchronized HelloZ, `actool`
  compiling what zcode wrote; 3,000 sets read off the main thread with only
  the thumbnails on screen decoded
* SwiftUI previews on a copy of HelloZ given a file with a named
  `#Preview` and a `PreviewProvider`: the gutter marks, following typing
  and undo, and in a second pane; a mark's click and ⌥⌘↩ (at a preview,
  outside one, in a file without previews), with the file saved first.
  Xcode is never opened: `ZCODE_XCODE_COMMANDS` (set for every launch)
  records the commands, which the checks compare (`xed <project>`, the
  wait for its window, `xed --project … --line …`, the canvas step)
* packaging: `scripts/package.py` builds and ad-hoc signs `zcode.app` in a
  temporary folder, whose `Info.plist` (version, document types, the
  login item's agent), icon and signature are checked, and the dry runs
  of Developer ID signing and notarization (no Developer ID certificate
  on the test Mac) print their commands without changing it; the cold
  launch of the bundle's binary against the bare one, in alternating
  rounds; then a copy of the bundle in a folder of its own: what Finder
  hands over (a percent-encoded package URL) opens in a second window,
  Install CLI links `zcode` into `ZCODE_CLI_DIR` and the link hands a
  path to the running zcode or has LaunchServices start the app (`open
  -a`, recorded by `ZCODE_XCODE_COMMANDS`), Open at Login toggles with a
  file standing in for launchd (`ZCODE_LOGIN_ITEM_FILE`; no real login
  item is ever registered), and Check for Updates… against
  `scripts/fake_update_feed.py` fails a wrong checksum, installs the next
  version (the same app with its version bumped, signed ad hoc) in the
  bundle's place and relaunches it with Restart Now (recorded), after
  which the installed app finds itself up to date. Nothing is installed
  in `/Applications` or `/usr/local/bin`, and LaunchServices is never
  told about the bundle
* Archive and Distribute App on a copy of HelloClassic, in the run's own
  Archives folder (never `~/Library/Developer/Xcode/Archives`): a real
  `xcodebuild archive` for any Mac (Sign to Run Locally, so it signs on any
  Mac) revealed in the Organizer with a report; an archive as Xcode leaves
  one listed beside it, with each platform's methods and export options;
  a real Copy App export, and a real Developer ID export refused for want
  of a team (pointing at Signing & Capabilities); HelloZ's archive
  refused for want of a team. Then through `scripts/fake_distribute.py`,
  which pretends (nothing is uploaded or sent to Apple): an App Store
  Connect upload and a TestFlight Internal Only export with their
  `ExportOptions.plist`, Developer ID notarized with a keychain profile and
  stapled (the profile kept in `settings.json`), a rejected notarization
  with Apple's log, missing credentials, a refused upload;
  `ArchiveProject` over MCP; Delete Archive; ⌥⇧⌘O
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
  stopped under lldb-dap with the console and an auto-continuing log message;
  Stop leaving no process behind, running, running under lldb-dap and paused
  at a breakpoint, and quitting zcode while the app runs
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
* the scale benchmarks' instrumentation on ZKit: the scan, tree, search
  and Open Quickly timings in `dump` and `bench status`, `bench frame /
  action / keys` and `type-latency`'s times to the frame drawn and the
  frame's work (the benchmarks themselves, on firefox-ios, are
  `scripts/bench.py`: `docs/scale.md`)
* with `--simulator`: unit tests, one test and one suite from their diamonds,
  the Smoke test plan, and run (with the scheme's arguments and environment)
  and debug on an iOS simulator (with the LLDB console)

The control channel also takes window screenshots
(`screenshot <path>`), which doesn't need Screen Recording permission.

## Known limitations

* Scale: the benchmarks (`docs/scale.md`) run on one project, firefox-ios's
  `Client.xcodeproj` (11,000 files); a monorepo of 100,000 files would
  hit the scan's cap (`MAX_FILES`) and hasn't been measured. The app's
  index build stops where firefox-ios needs its `bootstrap.sh` (a
  downloaded tool), which the benchmark doesn't run, so the compilation
  database covers the frameworks and packages but not the app's own
  files; the build server gives those their arguments anyway (from
  Xcode's index settings once the build has run, from build settings
  before). zcode opens with a project's first scheme in name order (a
  library here), and the benchmark picks the app's. The relative gates
  need a `master` with the benchmarks' control commands; an older one is
  measured through `dump`, and only the cold open, memory, readiness and
  keystroke-to-tick gates get a relative limit from it.
* Large files: whether a file gets a syntax tree is decided when it opens
  (and when its whole text is replaced, as on a reload), so a file that
  grows past 2 MB while edited keeps its tree, and one cut below keeps
  none until it's reopened. Without a tree there are no syntax colors
  (sourcekit-lsp's semantic tokens still color a Swift file), folds, or
  string and comment context for auto-closing and Enter. Files past 64 MB
  don't open. File watching: FSEvents doesn't watch the home folder itself
  or a folder that doesn't exist yet, so a file directly in `~` and a
  settings folder created later are polled as before; on a volume
  FSEvents doesn't journal (some network shares) changes are noticed
  within 30 s. An idle window's display link pauses, so GPUI no longer
  presents unchanged frames for a second after fast input (which keeps a
  ProMotion display at its top rate); input that changes anything draws
  as before.
* Build server: sourcekit-lsp doesn't prepare targets or index files
  itself (zcode doesn't offer `buildTarget/prepare`): the index is the
  index build's, so references, rename and call hierarchies reach only
  what that build compiled, while every open file still gets its
  arguments. A target outside the current scheme gets its arguments from
  `-showBuildSettings -alltargets` (for the destination's SDK when it
  supports it) or the last index build's log, and until anything is
  known a target's file is compiled alone with the Mac's SDK, as
  sourcekit-lsp does without a build server. Xcode's index arguments
  are empty for a target whose build Xcode can't plan (firefox-ios's app
  without its `bootstrap.sh`); such a target's arguments are made from its build
  settings (search paths, definitions, flags, the bridging header, the
  modules the index build made), without what custom build rules or
  generated sources would add. A file in several targets is compiled as
  its first's (an app's before its tests'), as sourcekit-lsp picks, and
  targets' dependencies aren't reported. An `.xcconfig` edited outside
  zcode is noticed only with the next project change or scheme switch.
* Packaging: Developer ID signing and notarization have only been
  dry-run (this Mac has an Apple Development certificate, no Developer
  ID), and the update feed's default URL is a placeholder until there's
  a release server (`ZCODE_UPDATE_FEED` at build time sets it). An update
  installs only when the bundle's folder is writable (otherwise Download
  opens the zip); there are no delta updates, and the update replaces
  the whole bundle. Finder's association was checked by inspecting
  `Info.plist` and by opening documents through `open -a`, not by
  registering the bundle as a default handler. Open at Login and the
  CLI's hand-off to LaunchServices need the bundle; a bare
  `target/release/zcode` says so.
* Predictive completion has two providers, the Claude API and a language
  server's `textDocument/inlineCompletion` (which sourcekit-lsp doesn't
  offer yet); there's no on-device model like Xcode's. A prediction is
  accepted whole (no word at a time), there's one at a time (no cycling
  through alternatives), and only in file tabs (not multibuffers or Code
  Review). The Claude API key is read from the settings or
  `ANTHROPIC_API_KEY` (zcode's environment, else the login shell's), not
  the keychain.

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
  not Xcode's project-versus-system distinction). Documents are sent as one
  range per change (every server zcode runs takes ranged changes), which
  completion's requests do on every keystroke.
* Web apps: the npm servers need Node.js (zcode looks for `npm` on your
  login shell's `PATH`). zcode installs a server's latest version the
  first time a file needs it (a few seconds to a minute, shown in the
  status bar) and doesn't update it afterwards: delete its folder in
  `~/Library/Application Support/zcode/language-servers` (then Restart
  Language Servers) for a newer one. JSON's schemas are fetched from the
  network by the server. A web app's servers are rooted at its outermost
  `package.json`, so a monorepo's packages share one each. Inlay hints and
  semantic highlighting come from a file's first server that has them (a
  Vue file's are Vue's server's). Vue's `{{ }}` interpolations aren't
  highlighted as JavaScript (directives' values are). There's no linked
  editing of a tag's pair, no ESLint fix-all on save, no running or
  debugging the web app (the terminal runs `npm run dev`) and no
  browser preview. Emmet works in markup, JSX, TSX and stylesheets (not in
  `.js` or `.ts` files), without CSS at-rules, vendor prefixes, BEM and
  other filters, or Emmet's other actions (Balance, Go to Edit Point,
  Update Tag); a snippet's repeated tab stops aren't linked (typing in
  one doesn't change the others).
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
* The agent panel keeps one conversation per window and doesn't keep it
  across relaunches (no thread history or `session/load`). zcode doesn't
  offer agents its terminal (`terminal/*`), so they run commands with
  their own tools, and it lets them write only inside the project. Images,
  audio, slash-command completion and agent-defined config options aren't
  shown, and a symbol is mentioned by its lines. Claude Code's and Codex's
  adapters (checked here with trivial prompts on a copy of a sample) read
  and write files with their own tools rather than through zcode, so
  their edits reach the buffer when their tool call reports its diff
  rather than as the write itself, and they don't see unsaved edits
  unless a file is mentioned (mentions always send the buffer's text).
  Gemini CLI wasn't run here (it needs a Google sign-in). Signing in to an
  agent is the agent's own flow (for Claude Code, `claude /login` in a
  terminal); the panel offers the agent's sign-in methods and a terminal.
  Agents' own files written outside `agent_servers`' commands (their
  configuration, credentials) are theirs; zcode never changes them.
* The MCP server needs zcode running and acts on projects zcode has open;
  it has no tools to open or close projects, edit files (agents have their
  own), search, or read build settings, and `zcode mcp` doesn't start zcode.
  It serves stdio only (no HTTP). Tools run one project's builds at a
  time, as the Product menu does: a build started by the user or another
  client replaces one a tool is waiting for (the tool says so). An agent
  in the panel that asks its own permission before an MCP tool (Claude
  Code does) is followed by zcode's prompt for the tools that change
  things. Checked with scripts/mcp_client.py and the fake ACP agent; not
  run against Claude Code, Codex, Gemini CLI or Cursor here (that would
  change their configuration on this machine).
* No Interface Builder, and no SwiftUI canvas of zcode's own: previews
  show in Xcode's canvas (Show in Xcode Canvas), which turns the canvas on
  through System Events and so needs Accessibility permission (without it,
  press ⌥⌘↩ in Xcode). The canvas keeps the preview tab last chosen rather
  than the one at the line. An Xcode that stops at its first-launch
  dialog never shows the file. A native preview on the simulator is
  measured in `docs/previews.md` but not built. Instruments is Apple's,
  which Profile records with and opens.
* Asset catalogs: the cheap edits only. Appearances, scales, devices,
  gamuts and other variations can't be added to a set (edit
  `Contents.json` with Open Quickly, or Xcode), properties such as Render
  As, Compression and Preserve Vector Data are shown, not edited, and new
  sets are image sets, color sets and folders (no app icons, data,
  symbols, AR, sticker or texture sets, though they're shown). A color is
  typed, not picked (no color panel), Display P3 and extended-range
  swatches are drawn as if sRGB, and system colors show their light
  values. Symbol images show their SVG as drawn (the template sheet, not
  one weight), data sets their file name, and sets of other kinds their
  files. Images are dragged in from Finder, not out of zcode or between
  wells. Undo lives with the tab: closing it (or quitting) forgets it, and
  removed files wait a day in zcode's cache folder. Dragging from Finder
  was checked through the window's file-drop events, not a real Finder
  drag.
* Profile: a recording couldn't be made on the machine zcode was built on
  (`xctrace record` needs Developer Tools access, `DevToolsSecurity
  -enable`, and failed there with "Error connecting to DTServiceHub"), so
  the flow was tested against a fake `xctrace`, and against the real one
  only up to that error (shown in the console, the partial trace opened).
  What the summary reads is `xctrace export`'s documented format (a
  `time-profile` table with ids and refs), not a real recording's export.
  Devices were profiled only through the fake `devicectl` and `xctrace`,
  never on hardware; a device's app is launched by its bundle identifier
  after `devicectl` installs it. The summary is Time Profiler's (and CPU
  Profiler's) heaviest functions only; other templates' data is in
  Instruments. Recording options (`--window`, extra instruments), attaching
  to a running app, Profile Test and the Profile action's working
  directory aren't offered; the scheme's Profile action is read, not
  edited.
* Archive and distribution: no upload or notarization was made from this
  machine; uploads to App Store Connect, TestFlight and Apple's notary
  service were only exercised against a fake `xcodebuild -exportArchive`
  and `notarytool` (`scripts/fake_distribute.py`), and the App Store,
  Release Testing, Enterprise, Debugging and Developer ID exports never
  ran for real (the Mac has no distribution certificate; only Copy App
  and a Developer ID export refused for want of a team did). Exports use
  automatic signing (`-allowProvisioningUpdates`): there's no manual
  signing with chosen certificates and profiles, no Validate App,
  Download Debug Symbols, App Thinning variants, distribution manifests,
  `distributionBundleIdentifier` or Custom flows, and no Mac installer
  package (`.pkg`) export. Notarization takes a keychain profile only
  (store it with `xcrun notarytool store-credentials`), notarizes an
  exported app (not a disk image or package) and isn't offered for
  uploads. The Organizer shows archives, not Xcode's crashes, energy,
  metrics or feedback; an archive's description is shown, not edited.
  Archive builds into Xcode's DerivedData, not zcode's index folder, and
  doesn't edit the scheme's Archive action. Swift packages can't be
  archived.
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
  recorded `devicectl` output and a fake `devicectl`. The USB attach and
  detach notifications that list devices again were seen to register, but
  no device was plugged in while testing them.
* Edit Scheme covers the Run and Test actions only (not Build, Profile,
  Analyze or Archive, nor pre- and post-actions; Profile and Archive read
  their actions from the file), and doesn't create,
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

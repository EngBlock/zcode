# Testing

```
cargo test                          # unit tests (zcode and crates/zcode-editor)
cargo test -- --ignored             # needs Xcode: sourcekit-lsp (also through `zcode bsp`), lldb-dap, and xcodebuild on an edited project
                                    # (network_* tests need network access)
cargo build --release && python3 scripts/e2e.py [--simulator] [--network] [--compare-apps]   # drives the real UI
python3 scripts/bench.py [--rounds 3] [--skip-index]   # scale benchmarks on firefox-ios, against main (docs/scale.md)
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

# Known limitations

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
  need a `main` with the benchmarks' control commands; an older one is
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
* Hidden tabs: sourcekit-lsp keeps open only the shown files, the four
  shown last and those with unsaved changes. A file closed there keeps the
  diagnostics it had (as Xcode's issues keep a file's that isn't open), so
  an error another file's edit causes or fixes in it shows once it's shown
  again; showing it again costs sourcekit-lsp its AST again (~1.3 s for a
  5,000-line file before its diagnostics are back), and Quick Help or
  completion in it waits that long. Its semantic tokens stay as they were
  meanwhile. Restarting the server clears a closed file's diagnostics until
  it's shown. The web's servers keep every open file.
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
  `Info.plist` and by opening documents through `open -a`;
  `scripts/default-app.sh` also registers the bundle as the default opener,
  which macOS 26.4 and later does only after the user confirms a type the
  first time it changes (there's no menu item for it in the app). Open at
  Login and the CLI's hand-off to LaunchServices need the bundle; a bare
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
  repository) gets no change bars. ⌘Z after Discard Change brings the
  lines back to the editor but not to the index: a change that was staged
  has to be staged again. A rename is followed only once it is staged
  (`git mv`, or both paths added).
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
* Live previews (Markdown and SVG): raw HTML in Markdown (`<img>`,
  `<details>`, `<br>`) isn't rendered as a browser would, images from the
  web don't load (zcode gives GPUI no HTTP client), and `#heading` links
  go nowhere. MDX is read as plain Markdown (JSX and expressions show as
  text). Scroll sync goes one way, editor to preview, by top-level block
  (a long list or table is one block, scrolled through in proportion);
  scrolling the preview doesn't move the editor. An SVG's `<text>` isn't
  drawn (resvg is built without fonts), nor are external images it
  refers to by URL. The Editor menu's Open Preview items are enabled for
  every file and do nothing for others (zcode's menus have no
  per-file state); Quick Actions hides them. A preview of a file whose
  last editor closed keeps the buffer open; Don't Save on that close
  puts the saved text back, which wasn't checked end to end (the e2e
  suite can't answer the dialog).
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
  need `xcodebuild test` (running only UI tests from a diamond uses it). On the machine zcode is developed on that fails ("The test runner hung
  before establishing connection") because of the simulator runtime, with
  either installed Xcode, independent of zcode.

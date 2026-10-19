# How it fits together

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
  gpui-base/        gpui-base 0.7.3, a text field's caret resting after 10 s without input (ZCODE-PATCH.md)
  gpui-component/   gpui-component 0.7.3 plus small patches (ZCODE-PATCH.md)
  gpui-pre-macos/   GPUI's macOS backend 0.3.9, its display link paused while a window is idle (ZCODE-PATCH.md)
scripts/
  e2e.py            end-to-end UI tests
  bench.py          scale benchmarks on firefox-ios (fetched into ~/Library/Caches/zcode-bench), gated against
                    a build of main made there (docs/scale.md)
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
(`vendor/gpui-pre-macos/ZCODE-PATCH.md`), and `gpui-base` (GPUI Kit's
widgets, Apache-2.0) with one that stops a text field's caret blinking
after 10 s without input, as zcode's editor's own copy of it does
(`vendor/gpui-base/ZCODE-PATCH.md`). Files changed on disk come from
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

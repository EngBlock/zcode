# UI fuzzing report, 2026-10-20

Fuzzing zcode at commit 347af03 found about 50 bugs: 1 crash, 3 that can
lose or mislead work, and a long tail of functional and visual issues.
Random fuzzing over about 6,700 steps found no other crash or hang.

**Method.** Each test instance was an isolated release build driven
through the debug control channel (`ZCODE_DEBUG_DIR`, `src/debug.rs`),
with its own config folder and a throwaway copy of `samples/`. Five agents
each probed one area: editor core, navigation and panes, project editors,
terminal/settings/previews, and source control and search. Random fuzzers
ran on ZKit, HelloClassic and HelloWeb. Every bug below was reproduced at
least twice; those marked ✔ were also re-checked independently.

In the repros, `$S` is the instance's samples copy and commands are
control-channel commands (`open`, `keys`, `type`, `click x y` in window
points, `action`, …). Screenshots referenced as `/tmp/zfuzz/…` are from
the session and may not survive a reboot.

## Crash

### ✔ Closing the breakpoint editor with Escape or Done aborts zcode

This crashes every time, however the editor was opened: Edit Breakpoint on
a line breakpoint, ⌥⌘\ (symbolic) or Create Exception Breakpoint.

- **Panic:** `cannot read zcode::workspace::breakpoint_editor::BreakpointEditor while it is already being updated` (gpui `entity_map.rs:156`).
- **Cause:** `BreakpointEditor::close` (`src/workspace/breakpoint_editor.rs:155`) calls `Workspace::dismiss_modal` (`src/workspace/mod.rs:813`). That calls `apply_breakpoint_edit` (`src/workspace/breakpoints.rs:302`), which reads the editor while it is still in its own update.
- **Repro:** `action zcode::CreateExceptionBreakpoint`, `wait 800`, `keys escape`.
- **Why e2e misses it:** `scripts/e2e.py` closes the editor only with the debug-only `breakpoint close` command. Clicking outside the popover closes it safely.

## Can lose or mislead work

### Project Settings fields are only saved on Enter

If you type a value and then tab away, click away or switch target, the
field shows the value but `project.pbxproj` is never written. The value is
gone after switching targets.

- **Repro (HelloClassic):** `action zcode::ShowProjectSettings`, `project-pane HelloClassic General`, `click 900 276` (Display Name), `type Blurry`, then `click 900 700`, `keys tab` or a click on another field. Switch to HelloClassicTests and back.
- **Expected:** leaving the field saves it. The code subscribes to `InputEvent::Blur` to commit (`rebuild_fields` in `src/workspace/project_settings.rs`), but the commit doesn't happen.
- The Version field behaves the same.

### Discard Change on a staged hunk leaves the change in the index

The buffer goes back to HEAD and the change bar disappears, but
`git diff --cached` still has the edit and the work tree has its reverse.
The next commit includes the change the user discarded, with nothing in
the gutter to show it.

- **Repro:** edit line 10 of Greeter.swift, `keys cmd-s`, `git stage-hunk 0`, click the hollow bar, `git popover discard`, `keys cmd-s`.
- **Expected:** the change is gone from both the index and the work tree, or Discard isn't offered for a staged hunk.

### Saving overwrites changes made on disk while the buffer is dirty

`save_buffer` never compares the file's modification time on disk.

- **Repro:** `settings open settings`, `type // note` (leave it unsaved), `action zcode::ToggleInvisibles` (disk now has `"show_whitespaces": "all"`), `keys cmd-s`.
- **Actual:** the file becomes `{}// note`; the toggle is reverted with no warning. A `.swift` file changed on disk while dirty is overwritten the same way.

## Functional, high impact

### ✔ The keyboard stops working after switching away from the Find navigator

After ⌘4 then ⌘5, every shortcut is ignored until you click a view:
⌘1–⌘9, ⇧⌘O, ⇧⌘Y, arrows, and dispatched actions.

- **Repro:** `click 800 500`, `keys cmd-4`, `keys cmd-5`, `keys cmd-6`. The navigator stays on Issues, and `keys cmd-shift-o` opens nothing.
- **Also:** hiding the focused Bookmarks filter (⌘3, click the filter, ⌘1) or the focused Terminal (⇧⌘Y) does the same.
- **Cause:** `show_navigator_tab` (`src/workspace/navigator.rs:121`) focuses `self.find.input` for Find, but nothing moves focus when the panel holding the focused input stops being drawn.

### ✔ Files created outside zcode never appear in the navigator or Open Quickly

This includes files created in zcode's own terminal.

- **Repro:** start on ZKit, `touch $S/Sources/ZKit/Outside.swift`, wait 8 s, `dump files` (still 5), then ⇧⌘O `Outside` (No Matches).
- **Cause:** `observe_window_activation` (`src/workspace/mod.rs:279`) only refreshes git. `rescan()` runs only after zcode's own file operations, a branch switch, or a relaunch.

### ✔ Swift package build errors lose their file and line

This toolchain's `swift build` prints `error: /path/File.swift:3:32 message`.
`parse_issue_line` (`src/build/log.rs:126`) only handles
`/path:3:32: error: message`, so these lines fall through to the generic
pattern.

- **Effect:** the errors are listed under "Build" as path-prefixed text, can't be clicked to their line, and don't mark the gutter. One includes a raw `FixIt(sourceRange: XCBuild.SwiftBuildMessage.DiagnosticInfo.SourceRange(…), textToInsert: ";")` dump.
- **Repro:** in ZKit, break `Greeter.swift`, ⌘S, ⌘B, then `dump issues`.
- The live errors from sourcekit-lsp still appear correctly under the file.

### Select Next Occurrence (⌥⌘E) never gets past the second match

- **Repro:** a file of `foo\nfoo\nfoo\nfoo\n`, `keys alt-cmd-e` four times, `type X`.
- **Actual:** `X\nX\nfoo\nfoo\n`; expected all four replaced.
- **Cause:** `select_next_occurrence` (`src/workspace/editing.rs` ~330) always searches from `state.selected_range()`, the first selection.

### Replace in Find & Replace replaces inside its own replacement

- **Repro:** `foo one\nfoo two\nfoo three\n`, `action zcode::FindAndReplaceInFile`, `type foo`, `keys tab`, `type foobar`, click Replace twice.
- **Actual:** `foobarbar one\nfoo two\nfoo three`; expected `foobar one\nfoobar two\n…`.
- **Cause:** `replace_current_search_match` (`vendor/gpui-base/src/input/editor/search.rs`) keeps the match index after recomputing matches, so it lands on the text just inserted. Replace All is fine.

### Find in Workspace searches the files on disk, not unsaved edits

- **Repro 1:** type `zzqq` into Greeter.swift without saving; Find in Workspace for `zzqq` gives 0 results.
- **Repro 2:** insert 7 lines at the top of Greeter.swift without saving, then search `func greeting`. The navigator lists line 9 (the line on disk), and clicking it opens buffer line 9 (`public struct Greeter {`).
- Show Find Results in Editor maps the excerpt correctly (line 16), so the navigator and the multibuffer disagree.

### Find doesn't move the cursor to the match

- **Repro:** open a 150-line file at line 1, `keys cmd-f`, `type needle` (match on line 150), `keys enter`, `keys escape`, `type X`.
- **Actual:** the view scrolls to line 150, but X is inserted at 1:1 and the view jumps back. Xcode, Zed and VS Code select the match.

## Functional, by area

### Editor

- **Move Line Down on a folded header** swaps it with the hidden line inside the fold, and the fold disappears. Repro: fold `init` in a copy of Greeter.swift (`mouse 314 194`, `click 314 194`), put the cursor on the header, `keys alt-down`. Result: `self.name = name` above `public init(name: String) {`.
- **Jumping to a line inside a fold leaves it closed.** ⌘L `6:9` with `init` folded puts the cursor on hidden line 6, drawn at the start of line 5.
- **Enter at column 1 of an indented line doubles the indentation.** `a\n    b\n`, cursor at line 2 col 1, Enter gives `a\n\n        b\n` with the cursor at col 5.
- **Typing `}` on an auto-indented line doesn't outdent.** There is no electric-brace logic; `{` + Enter does insert a correctly indented closing brace.
- **Indentation isn't detected.** In a 2-space TypeScript file (HelloWeb `web/src/main.ts`), Enter inserts 4 spaces and the status bar says "Spaces: 4". The docs don't promise detection, so this is a gap rather than a regression.
- **Cursor movement splits grapheme clusters.** In `a👍🏽b`, `right right` then `type Z` gives `a👍Z🏽b`. → stops between `e` and a combining U+0301, and ⇧→ in 👨‍👩‍👧‍👦 selects a lone U+200D.
- **Re-Indent (⌃I) takes one ⌘Z per changed line** (3 undos for a 3-line fix).
- **Typing over a selection takes two ⌘Zs.** Select all, `type abc`, ⌘Z leaves `a`; a second ⌘Z restores the text. The same happens after ⌥⌘E.
- **Tab with the completion menu open inserts spaces** instead of accepting (`self.gre` + Tab gives `self.gre    `). Xcode accepts on Tab.
- **⌘/ on an empty line does nothing**, including ⌘A ⌘/ in a file of blank lines; Xcode inserts `//`.
- **⌘/ shrinks a full-line selection.** Select lines 2–3 (ending at line 4 col 1), ⌘/: the selection now ends on line 3 without its newline, so a following ⌘D duplicates inline (`gammabeta`).
- **Turning on Wrap Lines with the cursor at the end of a very long line** scrolls to the top, leaving the caret off-screen.

### Navigation, panes and tabs

- **Go Back after closing the current tab does nothing.** Open GreeterTests.swift, then GreeterSuite.swift, ⌘W, ⌃⌘←: still on GreeterTests. Go Forward can't reach the closed file either. `step_history` (`src/workspace/navigation.rs`) pops the location already shown, and the closed tab's location is never saved.
- **More than about 10 side-by-side panes are placed off-screen, including the focused one.** 19× ⌃⌘T gives layout `h(0..19)` with `focused=19`, but only 10 panes are drawn. Opening a file puts it in invisible pane 19 while the status bar shows "Ln 1, Col 1".
- **The project navigator keeps highlighting the wrong file** when the active file's folder is collapsed, or the tab isn't a project file (Project Settings, `default-settings.json`). `reveal_active_in_tree` (`src/workspace/panes.rs:271`) never clears the selection when the row isn't found.
- **The "No Callers" popover outlives tab switches and Escape.** `action zcode::ShowCallers` in ContentView.swift, then ⇧⌘]: the popover stays over the next file; only a click in the editor dismisses it.
- **The tab strip doesn't scroll to the active tab when it overflows** (about 15 tabs in one pane).

### Project editors

- **Files zcode creates for Signing and Info don't appear in the navigator.** `signing add App Groups` writes `HelloClassic.entitlements` and adds it to the pbxproj group, but it isn't listed until something else forces a rescan. The same happens with `Info.plist` created from HelloZ's Info tab. The `ProjectSettingsEvent::Changed` handler (`src/workspace/events.rs`) never calls `rescan()`.
- **⌘Z doesn't undo Project Settings edits** (General, Build Settings, Info, capabilities). A removed build setting can only be brought back by typing it in again.
- **Case-only renames fail** with "“EXTRA.swift” already exists.": `rename_path` checks `target.exists()`, which is true on a case-insensitive volume. Asset catalog sets fail the same way (`move_item` in `src/xcassets/edit.rs`), and that error lowercases the name you typed (`edit.name.to_lowercase()`, `src/workspace/asset_catalog.rs:497`).
- **The plist editor accepts impossible dates and rolls them over:** `2026-02-30T00:00:00Z` is stored as 2026-03-02. Out-of-range times like 25:61:61 are rejected.

### Source control

- **A renamed file is compared against an empty file.** After `git mv`, the HEAD side is empty, every line shows a green "Added" bar, and the Commit sheet shows the whole file as a staged `@@ -1,0 +1,8 @@` hunk.
- **Discarding a hunk in a file with no final newline adds one.** Commit `let a = 1\nlet b = 2`, add `let c = 3` on a new line, `git discard-hunk 0`: the text ends in `\n`, the gutter shows no change, but git reports the file modified after saving.
- **An invalid regex in Find leaves the previous results in place.** With `.*` on, searching `greeting(` shows the error above the old `greeting` results, which stay clickable; Show Find Results in Editor opens `Find “greeting(”` with those stale results. `run_find` (`find.rs:84-90`) returns without clearing them.
- **Bookmark Line with Dialog loses what you type.** The dialog's field isn't focused because `show_modal_at` (`src/workspace/mod.rs:796`) moves focus to the dialog after `BookmarkDialog::new` focused the input. The breakpoint editor also opens with no field focused.
- **⌘\ and ⌃⌘' do nothing in a multibuffer:** `toggle_breakpoint` and `bookmark_line` rely on `active_item()`, which is None there.

### Settings and keymap

- **An unknown `buffer_font_family` is accepted silently.** `{"buffer_font_family": "NoSuchFontXYZ"}` reports no problem; the editor and terminal fall back to a proportional font, while the terminal still lays out an 11.2 pt cell grid, so its cursor sits well right of the prompt.
- **One malformed block in `keymap.json` discards every user binding.** `[{"bindings":{"cmd-shift-8":"OpenSettings"}},{"context":"Editor","bindings":["oops"]}]` gives `user_bindings: 0`. The docs say bindings that can't be made are reported and the rest apply; that works for errors inside a well-formed block.

### Previews

- **Text in SVGs is never drawn.** `<text font-family="Helvetica">` (or sans-serif, or no family) renders nothing; stderr says `No match for '"Helvetica"' font-family`. `src/xcassets/thumbnails.rs:240/254` builds `resvg::usvg::Options` with an empty font database.
- **Markdown images on the same line as another image shrink to text height** (a 400×300 image drawn at about 23×17 pt).
- **Missing, 0-byte or corrupt Markdown images show nothing**, neither alt text nor a placeholder, and a link to a missing file does nothing when clicked.

## Visual and minor

- **Narrow panes overlap:** at the 100 pt minimum, a pane's path bar draws over the next pane's, and the split/close buttons cover the tab title.
- **Find in Workspace cuts off long rows before the match**, so the matched text is invisible (Package.swift line 9 for query `e`).
- **Files outside the project** get the path bar "ZKit › / › Users › nathan › Library › …" and the inspector text "Swift packages define their targets in Package.swift". A Swift package file's inspector shows no Type or Location.
- **The About window shows a hammer glyph instead of the app icon**, and "About zcode" is missing from Quick Actions (`About` is added in `set_menus` but not in `commands()`).
- **A failed branch switch doesn't name the conflicting files:** the notification ends at "…would be overwritten by checkout:" because `git::error_message` keeps only the first `error:` line.
- **Authors on an untracked file** widens the gutter by about 190 pt but leaves it blank.
- **The Find navigator says "1 results in 1 files"** (`navigator.rs:531`).
- **The debug navigator's thread row reads "Thread 1407330 Thread 1".**

## What held up

- **Random fuzzing:** about 6,700 steps with no crash besides the breakpoint editor.
- **Build, test and run:** passes, Swift Testing and XCTest failures at their lines, run output in the console, overlapping ⌘R/⌘B/⌘U, breakpoints with stepping, variables, lldb console errors and Stop.
- **Bad input in project editors:** odd values in General, Build Settings, Signing, Info, plist and asset-catalog edits all kept `project.pbxproj`, plists and `Contents.json` valid (`plutil -lint`), and undo or restoring a value got back to the original bytes.
- **Editor basics:** auto-closing pairs, Enter auto-indent, undo/redo back to clean, ⌘D, ⌥↑/⌥↓, multi-line ⌘/, Jump to Line with bad input, add cursor below, folding with the arrow keys, completions with Return, Vim basics, ⌘S and reloads from disk.
- **Navigation:** Open Quickly with odd queries, tabs, splits, focus cycling, the assistant, history across jumps, every navigator's empty state, about 60 Quick Actions entries.
- **Source control:** change bars, next/previous change, stage/unstage, the Commit sheet's checks, branch name validation, 44 changes in the navigator.
- **Terminal:** colours, Unicode, floods, scrollback cap, `exit`, restore after relaunch.
- **Settings and themes:** broken JSON, wrong types and out-of-range values reported with line and column, last good settings kept.
- **Web:** TypeScript completion and diagnostics; the Add Package sheet with invalid URLs.

## Not bugs (harness artefacts)

- **Right-click** opens a native NSMenu whose tracking loop blocks the control channel, which looks like a hang.
- **A scripted zcode quits when its last window closes** (⌘W with no tabs), by design: `single_instance` is false when `ZCODE_DEBUG_DIR` is set (`src/app.rs:442`).
- **Screenshots of an inactive window** don't show selection or match highlights; the shared system clipboard made clipboard tests flaky.
- **Splitter drags** can't be tested reliably: the `drag` command sends its mouse events without a redraw in between. A very fast real flick might leave a splitter following the mouse; that needs a manual check.
- **The agent panel** starts a real coding agent when a message is sent. Isolated instances should point `agent_servers` at `scripts/fake_acp_agent.py`.

# Scale

How zcode behaves on a large app, measured and gated by
`scripts/bench.py`. The normal suite (`scripts/e2e.py`) stays small, fast
and offline; this one fetches a big project and takes a few minutes.

```
cargo build --release
python3 scripts/bench.py                    # 3 alternating rounds against main, then the index build
python3 scripts/bench.py --skip-index       # the latency rounds only (~5 minutes)
python3 scripts/bench.py --rounds 0 --skip-open --no-master --index-cap 1500   # the index build only
```

## The project

[firefox-ios](https://github.com/mozilla-mobile/firefox-ios) at commit
`0bd83b3` (main, 2026-10-17), opened as `firefox-ios/Client.xcodeproj`. That
is 10,908 files after `.gitignore` and zcode's skipped folders, 2,095 Swift
files under the project's folder, and 3,264 in the repository. The largest are a
9,427-line strings table and a 5,166-line view controller.

firefox-ios is under the Mozilla Public License 2.0. The script
shallow-clones the pinned commit into `~/Library/Caches/zcode-bench` (350
MB; `ZCODE_BENCH_CACHE` names another folder), checks the license's first
line, and checks that the checkout has no changes before and after a run.
zcode only reads it, and nothing of it is copied into this repository.

## What's measured

Each round opens the project in a fresh zcode, with its own configuration
folder, and drives it through the control channel (`bench …` in
`src/debug.rs`):

| Gate | What |
|---|---|
| `open_pixels_ms` | Cold open of the project with `--startup-bench`: process start to the window's content on screen |
| `scan_ms`, `tree_ms`, `listed_at_ms` | The file scan (background), building the navigator's tree (main thread), and when the navigator has the files, from `main()` |
| `navigator_frame_ms` | A redraw with the project navigator |
| `open_quickly_show_ms`, `open_quickly_key_ms`, `open_quickly_match_ms` | Showing Open Quickly, each keystroke of `BrowserViewController`, and the fuzzy match over every path |
| `find_common_ms`, `find_many_ms`, `find_miss_ms` | Find in Workspace for `viewDidLoad` (~250 matches in ~120 files), `the` (the 5,000-match cap), and a word found nowhere (every file read) |
| `find_frame_ms` | A redraw with 5,000 results in the Find navigator |
| `open_footprint_mb`, `open_resident_mb` | Memory after the open: the lowest sample over 12 s idle |
| `search_footprint_mb`, `search_resident_mb` | The same after the 5,000-result search, its results on screen |
| `lsp_ready_s`, `tokens_s` | From opening the view controller to sourcekit-lsp ready and to the file's first semantic tokens |
| `typing_ms`, `typing_big_ms` | A keystroke's work (below) while typing a line in the view controller and in the strings table |
| `typing_tick_ms`, `typing_big_tick_ms` | Keystroke to the start of the next frame, wall-clock |
| `typing_huge_ms` | A keystroke's work typing a line near the end of a 50,000-line Swift file (generated into the cache, `huge/Huge.swift`, opened next to the project) |
| `pagedown_json_ms` | The work of five Page Downs in a 15 MB JSON file (`huge/huge.json`), median of 5 |
| `typing_minified_ms` | A keystroke's work typing at the end of a 3.3 MB minified script, one line (`huge/min.js`) |
| `pagedown_minified_ms` | The work of five Page Downs in that script, median of 5 |
| `lsp_footprint_mb` | sourcekit-lsp with its `SourceKitService` helpers at the end |

**Work, not ticks.** Frames are gated on their *work*: the input's
handling plus the draw and present of the frame it causes. The display's
ticks come every 8 ms on a quiet machine but every ~33 ms under load,
when macOS drops to 30 Hz. A wall-clock keystroke-to-frame time then reads
~31 ms for any build, so it is gated only loosely. The time to the frame
on screen is reported beside each work figure.

**The index build** comes after the rounds, in a session of its own.
zcode opens a project with its first scheme in name order. Here that is
`Account`, a static library that only builds for the Mac, and
firefox-ios's packages have no Mac slice. So the script picks the app's
scheme, `Fennec`, in ⌃0's picker, as a user would. zcode then picks an
iPhone simulator. `bench index` starts the background index build, as a
save does. The script waits for it to start and end, up to
`--index-cap` seconds (600 by default). With a database written, it
opens a file the database covers and times sourcekit-lsp with it, then
the app's view controller, which the database lacks: when its semantic
tokens come, how many, and where its arguments came from (`bsp options`,
zcode's build server). The phase runs with each binary. This is
reported, not gated: the build's time is the toolchain's.
During the rounds no index build runs when the project opens
(`ZCODE_INDEX_BUILD=0`), so it doesn't load the machine while they
measure.

zcode keeps a project's compilation database and derived data in
`~/Library/Caches/zcode/projects`, whatever its configuration folder. So
the index phase removes any database it finds before building, which
makes each run's build a first one. It also removes the database it
wrote once it's measured, so the next run's rounds open the project
without one. A round that finds one anyway (after an interrupted run)
says so, since sourcekit-lsp's numbers then differ.

## Gates and noise

The machine these run on is often loaded by other work, with load averages
of 10–70. To keep that from failing a build that didn't regress:

* The branch and a release build of `main` alternate round by round
  (`main` is built once per commit in the cache). Each metric's median
  over the rounds is compared.
* A gate fails when the branch's median is over a generous absolute
  ceiling, or over main × factor + slack: ×1.15 + 25 ms for the cold
  open, ×1.5 + a few ms for frames, ×1.3 + 50–60 MB for memory, ×1.5 +
  100 ms for searches, ×2 + 5 s for sourcekit-lsp. Both limits are in the
  `GATES` table at the top of the script.
* Every result carries the load average when it was taken.
* A `main` from before the benchmarks' control commands is measured
  through `dump`. Its index build is stopped as it starts, since it can't
  be turned off there. Only the cold open, memory footprint, sourcekit-lsp
  and keystroke-to-tick gates get a relative limit from it. From the next
  merge on, main has the commands and every gate is relative.

`python3 scripts/bench.py --self-test` checks the gate arithmetic.

## Baselines

M4 Max, macOS 27.2, release builds; 3 alternating rounds of the change
against the build before it (`237031c`); load average 1.6–2.9 throughout.

| | After | Before |
|---|---|---|
| Cold open of the project, first pixels | 264 ms | 290 ms |
| File scan, 10,908 files | 77 ms | — |
| Navigator tree | 5.7 ms | — |
| Navigator has the files, from `main()` | 224 ms | — |
| A redraw, project navigator | 2.1 ms work, 3.4 ms on screen | — |
| Open Quickly shown | 5.0 ms | — |
| Open Quickly keystroke | 2.8 ms work, 6.9 ms on screen | — |
| Open Quickly's match over 10,908 paths | 0.3 ms | — |
| Find `viewDidLoad`, 249 matches | 101 ms | — |
| Find `the`, the 5,000 cap | 3.7 ms | — |
| Find a word in no file | 100 ms | — |
| A redraw, 5,000 results in the Find navigator | 2.3 ms work, 3.6 ms on screen | ~80 ms |
| Footprint / resident, idle after the open | 116 / 166 MB | 117 MB / — |
| Footprint / resident, after the search | 405 / 175 MB | 547 MB / — |
| sourcekit-lsp ready, first semantic tokens | 0.4 s, 1.5 s | 0.4 s, 1.6 s |
| Typing in 5,166 / 9,427 lines, work | 4.3 / 3.7 ms | — |
| Typing, keystroke to the next tick | 3.8 / 3.8 ms | 4.2 / 4.2 ms |
| sourcekit-lsp with its helpers | 316 MB | 280 MB |

The earlier Find navigator redraw was measured before the fix
(the same code), in the profiling below. The full
report is written as JSON to `~/Library/Caches/zcode-bench/reports/`.

The index build, from a clean derived-data folder (load average 6–12):

| | |
|---|---|
| `Fennec` for the iPhone 17 simulator, started | 0.4 s after `bench index` |
| Ended | 58 s, `xcodebuild` exit 65 |
| Compilation database | 1,670 files: BrowserKit, the Rust components' Swift wrappers, the packages; not the app's |
| sourcekit-lsp restarted with it, `Strings.swift`'s semantic tokens | 6.3 s |

The build stops at the app target's Nimbus Feature Manifest script, which
runs `bin/nimbus-fml.sh`. firefox-ios's `bootstrap.sh` downloads that
tool, and the benchmark doesn't run it: it would fetch and run
third-party tooling and write into the checkout. Everything the app
depends on compiles first, and the database keeps those commands.

## The build server

Foundations 3 hands sourcekit-lsp its arguments from zcode's build server
(`src/bsp`) instead of the compilation database. Measured with the same
script, against the build before it (`7afb09d`), 3 alternating rounds, load
average 2–7:

| | After | Before |
|---|---|---|
| After the index build, the app's view controller (not in the database) | semantic tokens after 8.2 s, 4,945 of them; arguments: Xcode's index settings | no semantic tokens in 120 s |
| Rounds with no database (the rounds' scheme is `Account`, a Mac library) | ready 0.8 s, first tokens 1.9 s | ready 0.4 s, first tokens 1.5 s |
| Rounds that found a database at open (left by an interrupted session) | first tokens 1.9–2.0 s | no tokens in any round |
| sourcekit-lsp with its helpers, end of a round | 246–322 MB | 228–311 MB |

Every other gate held, within a few percent of the build before. In the rounds the
app target isn't in the scheme: until Xcode has answered, its files are
compiled alone with the Mac's SDK (as sourcekit-lsp does without a build
server, hence the same first tokens), then with the settings
`-showBuildSettings -alltargets` gives it (its own SDK, here iPhoneOS):
in a separate check, 633 tokens from the file alone at 2.6 s, then 3,287
once those arrived (~30 s, most of it `xcodebuild` on a loaded machine).
Getting there turned up three things a target never built needs: files
it lists that a build generates (`Client/Generated/FxNimbus.swift`) left
off the command line, since one missing input makes the compiler give up
on the module; the folders of the project's headers, which Xcode's header
maps make reachable (the app's bridging header imports them by name); and
a refresh of semantic tokens when sourcekit-lsp reopens a file with new
settings, which it only asks clients that pull diagnostics for. Once the
index build has run, `xcodebuild -showBuildSettingsForIndex` has the
app's arguments too (before it, it lists the app's 1,231 files without
any), and those come first. The index phase took 55–62 s either way;
sourcekit-lsp isn't restarted when it ends, it reads the index store as
the build writes it.

## What profiling found

**The Find navigator laid out every result.** With 5,000 results, every
redraw of the window took ~80 ms (`bench frame`: 79–90 ms to the frame on
screen against 2–8 ms before the search). A `sample` of the main thread
during forced redraws showed it in taffy's layout under the window's draw,
for a list of ~5,600 rows (a header per file and its matches) built as
elements on every frame. The heap grew to match: `footprint` showed 162 MB
of small allocations with the results shown, against 48 MB after the fix.
The navigator now keeps the rows (a file's header, then up to 100 of its
matches) and lays out only those on screen in a uniform list. A redraw
takes 2.3 ms of work, and the footprint after the search drops by ~140 MB
against the build before.

**An interrupted first index build left no database.** zcode's
compilation database comes from the compile commands its background
index build logs. If zcode quits while that first build runs, its
products stay in the project's `IndexDerivedData`. The next build is then
incremental: it compiles little or nothing, and the database is never
written, leaving sourcekit-lsp without the project's arguments. That
happened here after the exploratory sessions. While there's no database,
an index build now clears the earlier build's products first, keeping the
package checkouts and the index store.

**Checked, and not bottlenecks:**

* The scan (`ignore`'s walk, sorted) takes 77 ms on a background thread.
  The tree takes 6 ms on the main thread, and the folders start collapsed
  above 400 files.
* Open Quickly's matching (`nucleo`) scores 10,908 paths in 0.3 ms per
  keystroke.
* Find in Workspace reads every file in 100 ms on 8 threads. A query
  with many matches stops at the 5,000 cap within milliseconds.
* Typing in a 9,427-line file costs 3.7 ms of work per keystroke, the
  same as in a small one. Opening and closing that file four times leaves
  the heap where it was (58 MB of small allocations, 188–190 MB
  resident), so buffers are let go.
* sourcekit-lsp answers for an open file from the file alone in under 2
  s, without waiting for the index build.

**Fixed by the build server (Foundations 3):** with a database that
lacks a file (the app's files here), sourcekit-lsp returned no semantic
tokens for it; without a database it answered from the file alone. Any
project whose index build stops partway hit this. zcode's build server
(`src/bsp`) now gives every file of a target its arguments; see below.

## Performance batch 1

Four fixes from a profile of a release build from before the change, each measured
against that build in alternating launches (M4 Max, macOS 27.2; the load
average beside each, as other work shares the machine). The drivers are
the control channel's `bench keys`, `type-latency` and `--startup-bench`.

| | Before | Now | How measured |
|---|---|---|---|
| Five Page Downs in a 15 MB JSON file, work | 31 ms (23 ms in `bench.py`) | 19 ms (15 ms) | median of 4 launches × 5, load 5–12 |
| CPU to open 30 firefox-ios Swift files | 1.39–1.46 s | 0.99–1.04 s | 3 launches each, load 3–9 |
| ZKit with 4 Swift tabs restored, first frame / first pixels | 218 / 264 ms | 201 / 246 ms | median of 15, load 9–20 |
| Welcome window, first frame / first pixels | 198.0 / 233.9 ms | 195.4 / 232.5 ms | median of 15, load 2–5 |
| Typing near the end of a 50,000-line Swift file, work per keystroke | 7.8 ms (6.7 ms in `bench.py`) | 1.85 ms (2.9 ms) | median of 3 launches × 3, load 4–7 |
| Typing near its start | 8.4 ms | 3.1 ms | median of 4 launches × 3, load 4–7 |
| Typing in the 9,427-line strings table (`bench.py`) | 4.7 ms | 3.2 ms | 1 round, load 2–3 |
| `devicectl list devices` runs in 60 s, idle HelloZ window | 12 | 1 | 2 launches each, load 2–10 |

* **Fold lookups scanned every fold.** The gutter asks, for each visible
  row on every frame, whether a fold starts there and whether it's folded;
  both were linear scans of the candidates (hundreds of thousands in a large
  JSON file). Both lists are kept sorted by start line, so they're binary
  searches now, as are folding and unfolding.
* **Every editor compiled its language's queries.** gpui-component's
  `SyntaxHighlighter` compiled the highlight, injection and locals queries
  (~13 ms of CPU for Swift) for each editor, and each parse with injections
  recompiled the injected languages' highlights. A patch shares one
  compiled copy per language (`vendor/gpui-component/ZCODE-PATCH.md`), and a
  project's restored tabs' languages are compiled on a background thread
  while its window is created: the shown tab's editor builds its
  highlighter before the window appears, and that took ~12.6 ms of the
  main thread, 3 µs now. Inactive restored tabs never built highlighters
  (they're made at an editor's first render), so the gain at launch is the
  one shown tab's.
* **A keystroke diffed the whole file.** Completion syncs the document
  before asking, and the change sent was found by comparing the text
  last sent with the current one byte by byte (ropey's `Bytes` iterator):
  ~3 ms a keystroke in a 1.2 MB file. The change is now the editor's own
  edits since the server's text, from an `EditJournal` the editor fills as
  each edit is applied (events come too late, after the provider runs),
  checked to make the current text; when they don't, the comparison runs by
  chunk, skipping the ones both ropes share (an edited rope keeps the
  chunks it didn't touch). Edits that run into each other (typing a word)
  are merged; two cursors' edits stay two small changes instead of one
  range spanning everything between them.
* **An edit's fold candidates walked the whole top level.** After each
  edit the editor asks the syntax tree for the folds in the edited range,
  and the walk visited every child of the root to check it against the
  range (20,000 in the generated file). It's one tree-cursor descent now,
  entering each level at the first child reaching the edit and leaving
  after the last one starting in it.
* **Devices were listed every 5 s.** Each `devicectl list devices` costs
  ~60 ms of CPU and wakes CoreDevice. Devices are now listed when an Apple
  USB device attaches or detaches (IOKit notifications, `src/build/usb.rs`),
  when the window becomes active, when the destination menu or Choose
  Destination opens, and otherwise once a minute. No device was plugged in
  during testing; the notifications were seen to register.

## Performance batch 2: long lines

With a minified script open (3.3 MB on one line), every frame after an edit
shaped the whole line through CoreText (`TextElement::prepaint` →
`shape_line`; ~78% of the main thread in a `sample` while typing) and then
walked all 3.3 million glyphs to paint the few hundred on screen (~18%).
The layout held ~210 MB for it. Turning soft wrap on shaped the line again
in pieces to find each row's break, several times a row (5.5 s), and
typing then re-wrapped all of it (2.8 s a keystroke).

Now a line over 4 KB is measured rather than shaped: the x of a checkpoint
every ~1 KB, summed from the font's width for each character, shifted
rather than measured again after an edit. A frame shapes only the 256-byte
tiles across the text area and around the caret (soft wrap on: the rows on
screen and around it) and styles only those bytes; carets, selections,
clicks and vertical movement measure from the checkpoints. Soft wrap wraps
the line by the same widths and re-wraps an edit until its rows meet the
old ones. Selections, search highlights and decorations on such a line are
laid out on screen only. (`docs/editor-engine.md`, Long lines.)

Profiling the result turned up two more walks of the line, at its end:
LSP positions (completion syncs the document on each keystroke) counted
UTF-16 units by walking the line, ~25 ms; and the status bar's column
counted characters the same way on every render, ~15 ms. Both read the
rope's counts now.

Measured against a release build from before the change in alternating launches (M4 Max,
macOS 27.2; 3 launches each, medians, load average 2.6–5), on
`/tmp/zbig/min.js` (`var a0=function(b){return b*0+Math.sqrt(0)};…` for
60,000 functions):

| | Before | Now |
|---|---|---|
| Typing at the line's start, work per keystroke | 36.8 ms | 3.2 ms |
| Typing at its end | 63.0 ms | 7.0 ms |
| Five Page Downs, work | 61.5 ms | 12.8 ms |
| ⌘↓ to the end | 32.9 ms | 7.0 ms |
| A redraw | 16.4 ms | 6.8 ms |
| Resident memory with it open | 626 MB | 331 MB |
| The same file as plain text (no syntax tree), resident memory added by opening it | 225 MB | 14.5 MB |
| Editor ▸ Wrap Lines on, to the frame on screen | 5.5 s | 58 ms |
| Typing at the end with soft wrap on | 2.8 s | 7.0 ms |

"Work" is the control channel's: the input's handling plus the frame's
draw and present, which includes waiting for a drawable from the window
server; with the work itself on the main thread, a keystroke at the
line's end measured 1.3–1.5 ms in a `sample`d run (the e2e check's run:
1.3 ms against 2.7 ms in a 200-line file), and the 7 ms medians above are
mostly that wait (a redraw's 6.8 ms is too). The rest of the 331 MB is
the syntax tree tree-sitter builds for the file (~190 MB), which batch 3
takes on.

The ordinary paths don't change: typing near the end of the 50,000-line
Swift file 1.7 ms of work a keystroke against 1.7 ms before, in a
small Swift file 1.5 against 1.5 ms (8 alternating launches each, load
3–5); cold launch, the welcome window's first pixels at 235.6 ms against
243.6 ms (median of 15, alternating, load ~2).

`scripts/bench.py` gates typing at the end of a generated minified script
and Page Down in it. One round against the build before (load average 3–4):
`typing_minified_ms` 2.2 ms against 56.4 ms, `pagedown_minified_ms` 11.2 ms
against 68.3 ms, and every other gate within a few percent of the build before
except `listed_at_ms` (the background file scan, which the change doesn't
touch: 431 against 244 ms in that round, the new build's first and cold; 250
against 284 ms over two more rounds).

## Performance batch 3: idle wakeups and large-file memory

Measured against a release build from before the change (`ca396fe`) in alternating
launches (M4 Max, macOS 27.2; the load average beside each).

**An idle window woke at the display's rate.** GPUI's macOS backend only
stopped a window's display link while the window was occluded, so a
visible window with nothing to draw woke the main thread on every refresh
(120 Hz here) to find nothing dirty. GPUI's core already calls a
platform's frame waker whenever a window gains frame demand (a view
notified, a refresh, a next-frame callback or animation frame, a throttled
frame to retry), for platforms that stop asking for frames; the macOS
window didn't provide one. A patch to the vendored backend
(`vendor/gpui-pre-macos/ZCODE-PATCH.md`) does: after four frame requests
in a row with no demand (~25 ms) the window's display-link subscription
stops, and the waker starts it again and asks for a frame on the main
queue at once, so the first frame after a pause doesn't wait for the
restarted link.

**What was left woke for nothing too.** With the link paused, a `sample`
of an idle HelloZ window showed the main thread drawing every few seconds:
the status bar's memory figure, from the last sample, flipped between
two values after each redraw (the compositor's memory comes and goes with
a frame), and each flip redrew the window. It now shows the 30 s idle
baseline. Then the polls: the settings files every second, the git
directory every 2 s, the open files' times every 3 s, and (with
sourcekit-lsp connected) the build server's projects and synchronized
folders, walked, every 2 s; and a 1 s ticker for the activity view that
only redrew during builds. The polls are now woken by FSEvents
(`src/fs_watch.rs`: one stream through `notify`, covering each project's
tree whole and the folders of files outside it, made on a thread of its
own that starts 1.5 s after the window), each keeping its check and
polling at its old interval only where the stream can't cover its files
(the home folder itself, a folder that doesn't exist), and every 30 s
anyway; the ticker runs only while a build or run is busy. The memory
sampler (every 2 s, one `proc_pid_rusage`) stays.

Idle over 60 s, no file open (two alternating launches each, medians):

| | Before: CPU, interrupt wakeups, context switches | Now |
|---|---|---|
| Welcome window | 429–566 ms, 8,700, 17,830 | 11 ms, 41, 210 |
| ZKit (Swift package) | 722–761 ms, 8,727, 18,250 | 35–50 ms, 49, 280 |
| HelloZ (iOS app) | 734–762 ms, 8,735, 18,270 | 36–42 ms, 47, 270 |

(load 1.3–2.1; `proc_pid_rusage` before and after, no control channel).
The earlier ~145 wakeups a second are the display link's (120 Hz) and the
polls'; now less than one a second remains, mostly the memory sampler.

A keystroke after a pause draws at once rather than at the restarted
link's first tick: 20 single keystrokes each after 300 ms idle reached the
screen in 6.8–6.9 ms against 7.4–7.7 ms before (11.5–12.1 ms when
waiting for the link instead, which is why the waker asks for a frame);
the pause needs four frame requests over 33 ms without demand, since a
restarted link can tick in a burst.

With an editor focused and its caret blinking (the e2e check, whose
control channel reads a file every 150 ms): 24 interrupt wakeups a second
against 161 before. A file changed outside zcode is read again in
~0.2 s (up to 3 s before), a saved settings.json applied in ~0.2 s (up to
1 s). Typing, caret blink, animations, terminal output, scrolling and
resizing go through the waker like any other demand; the e2e suite's
typing, terminal and pane checks pass unchanged.

**Syntax trees were 25–57× the file.** A file over 2 MB, or over 256 KB
and mostly on lines past 10,000 bytes (where syntax colors already stopped),
now gets no tree-sitter tree (`crates/zcode-editor/src/base/large_file.rs`).
It keeps its language: comments (⌘/), auto-closed brackets, indentation
rules, Vim's text objects (all text-based) and its language servers; it
has no syntax colors, folds, or string and comment context (which
auto-closing then treats as code), and the status bar says "Large file" or
"Minified: no syntax highlighting". Test and preview discovery, which parse
Swift on their own, skip such files too.

**The soft-wrap index cost ~110 bytes a line.** Its `SumTree` held a
`LineItem` per line (length, indent, a `SmallVec` of rows, the long-line
measurements) and its summary. Lines that are one row each are now runs of
up to 128 lines keeping their count, bytes and longest; a line that wraps
or is long is kept whole. An edit re-reads the lengths of the lines of the
runs it starts and ends in from the rope.

**~0.8 MB of fixed memory per open tab.** `MallocStackLogging` with
`malloc_history`, 40 firefox-ios tabs against none: 13.7 MB of the 31.7 MB
was allocated in `TextElement::prepaint`, one ~350 KB block per editor,
the `Vec<LineLayout>` of its last frame. A `LineLayout` was 9.1 KB, 6 KB
of it a copy of the layout's two shaped whitespace marks (GPUI's
`ShapedLine` is ~3 KB, mostly 32 inline decoration runs). The marks are
now shared by the layout's lines, and an editor in a tab no pane shows
lets go of its last layout (`release_layout`, when the shown tab changes).
What's left per tab is mostly its syntax tree (~7.8 MB for the 40 files'
5,444 lines) and the window's own layout and scene.

Heap added by opening a file (`heap`, fresh launches, load 3–13):

| | Before | Now |
|---|---|---|
| 15 MB JSON, 1.2M lines (no tree now) | 514 MB | 22 MB |
| 3.3 MB minified script, one line (no tree) | 194 MB | 5 MB |
| 1.2 MB Swift file, 50,000 lines (keeps its tree) | 61 MB | 55 MB |
| 40 firefox-ios Swift tabs | 31.7 MB | 15.0 MB |
| Resident added by the JSON file | 633 MB | 130 MB |
| Resident added by the minified script | 213 MB | 23 MB |

Files up to 64 MB open now (16 MB before): a 60 MB log adds 88 MB of heap
and types at 3.1 ms of work a key, five Page Downs 14 ms. Past 16 MB a file
goes to no language server and gets no change bars (a line diff at every
pause in typing), so nothing changes for the files that opened before.

The ordinary paths, alternating launches: typing near the end of the
50,000-line Swift file 1.65–1.74 ms of work a key against 1.82–3.12 ms; in a
small Swift file 1.6–3.2 against 1.4–1.7 ms (medians of 10 keys, the
spread bimodal on both); five Page Downs in the JSON file 12.9–18.5 ms
against 19.0–19.6 ms (load 1.5–3.4). Cold launch, the welcome window's
first pixels: 246.0, 244.0, 245.2 and 241.8 ms against 242.2, 241.2, 244.8
and 243.9 ms (four alternating rounds of 15–20, load 2–5).

`scripts/bench.py`, one round against that build (load 4–5): every gate
passed; typing 4.2 / 3.3 / 3.0 ms (strings table, view controller,
50,000-line file) against 4.4 / 3.3 / 3.0 ms, Page Down in the JSON file
12.1 against 13.8 ms, in the minified script 9.6 against 9.5 ms. The idle
footprint after the open read 141 against 118 MB: how many drawables Core
Animation still held 15 s in (one more window-sized IOSurface); 60 s in,
both held six, 128 against 133 MB, resident 164 against 170 MB.

Two e2e checks that failed before pass now. "Packages: browse a
package's sources" failed since the build server: ⌘B stopped the
background index build and nothing started it again, so the index
build's products (which sourcekit-lsp's arguments point at) lacked the
package's modules and Jump to Definition found nothing; a build that stops
it now starts it again when it's done. "Predictions: typing latency…"
compared the time to the next display tick, which depends on where each
keystroke lands in the display's cadence. It now takes the keystroke's
work or the time to the frame on screen: a keystroke's frame either waits
for the next tick and then draws, or, typing in step with the display,
draws at once and waits to present, which the work counts and the tick
doesn't (seen with the change in about one run in two: work 7 ms with 0.3 ms
to the tick, against 3 ms and 4 ms; on screen 7.0 ms either way, as on
before).

## About the memory numbers

The footprint after the search includes over 200 MB of compositor memory that
macOS attributes to a window while it redraws. The Find field's caret
blinks, so the window keeps redrawing (see "About memory" in the README).
Resident memory, 175 MB, is the better figure for what the search
costs. Both are gated.

## Limits

* One project. A monorepo past 100,000 files would hit the scan's cap
  (`MAX_FILES`) and hasn't been measured.
* The app's own files don't compile without firefox-ios's
  `bootstrap.sh`, so the index build's time is that of its dependencies,
  and sourcekit-lsp with a database is timed on a framework's file.
* zcode's choice of the first scheme by name hasn't been checked against
  Xcode's choice for a project with no user data.
* Cold open is measured with `--startup-bench`, which opens the project's
  window. It doesn't include the scan, whose numbers are separate.

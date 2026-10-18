# Scale

How zcode behaves on a large app, measured and gated by
`scripts/bench.py`. The normal suite (`scripts/e2e.py`) stays small, fast
and offline; this one fetches a big project and takes a few minutes.

```
cargo build --release
python3 scripts/bench.py                    # 3 alternating rounds against master, then the index build
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

* The branch and a release build of `master` alternate round by round
  (`master` is built once per commit in the cache). Each metric's median
  over the rounds is compared.
* A gate fails when the branch's median is over a generous absolute
  ceiling, or over master × factor + slack: ×1.15 + 25 ms for the cold
  open, ×1.5 + a few ms for frames, ×1.3 + 50–60 MB for memory, ×1.5 +
  100 ms for searches, ×2 + 5 s for sourcekit-lsp. Both limits are in the
  `GATES` table at the top of the script.
* Every result carries the load average when it was taken.
* A `master` from before the benchmarks' control commands is measured
  through `dump`. Its index build is stopped as it starts, since it can't
  be turned off there. Only the cold open, memory footprint, sourcekit-lsp
  and keystroke-to-tick gates get a relative limit from it. From the next
  merge on, master has the commands and every gate is relative.

`python3 scripts/bench.py --self-test` checks the gate arithmetic.

## Baselines

M4 Max, macOS 27.2, release builds; 3 alternating rounds of this branch
against `master` (`237031c`); load average 1.6–2.9 throughout.

| | Branch | Master |
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

Master's Find navigator redraw was measured on this branch before the fix
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
script, branch against `master` (`7afb09d`), 3 alternating rounds, load
average 2–7:

| | Branch | Master |
|---|---|---|
| After the index build, the app's view controller (not in the database) | semantic tokens after 8.2 s, 4,945 of them; arguments: Xcode's index settings | no semantic tokens in 120 s |
| Rounds with no database (the rounds' scheme is `Account`, a Mac library) | ready 0.8 s, first tokens 1.9 s | ready 0.4 s, first tokens 1.5 s |
| Rounds that found a database at open (left by an interrupted session) | first tokens 1.9–2.0 s | no tokens in any round |
| sourcekit-lsp with its helpers, end of a round | 246–322 MB | 228–311 MB |

Every other gate held, within a few percent of master. In the rounds the
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
against master.

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

Four fixes from a profile of a `master` release build, each measured
against that build in alternating launches (M4 Max, macOS 27.2; the load
average beside each, as other work shares the machine). The drivers are
the control channel's `bench keys`, `type-latency` and `--startup-bench`.

| | `master` | Now | How measured |
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

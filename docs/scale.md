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
opens a file the database covers and times sourcekit-lsp's restart with
it. This is reported, not gated: the build's time is the toolchain's.
During the rounds no index build runs when the project opens
(`ZCODE_INDEX_BUILD=0`), so it doesn't load the machine while they
measure.

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

**Found, not fixed:** with a database that lacks a file (the app's files
here), sourcekit-lsp returns no semantic tokens for it. Without a
database it answers from the file alone. Any project whose index build
stops partway hits this. A build server that hands sourcekit-lsp
settings for every file (Foundations 3, the Build Server Protocol server)
is the place to fix it.

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

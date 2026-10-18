#!/usr/bin/env python3
"""Scale benchmarks: zcode on a large open-source app, with gates.

    cargo build --release && python3 scripts/bench.py [--rounds N] [--master <zcode>|--no-master]
                                                       [--skip-open] [--skip-index] [--index-cap <s>]
                                                       [--only <text>[,<text>…]] [--report <file>]
                                                       [--project <path>] [--self-test]

Fetches firefox-ios (Mozilla Public License 2.0; read, never copied into
this repository) at a pinned commit into ~/Library/Caches/zcode-bench, builds
`master` there for the relative gates, then drives both binaries through the
control channel (`ZCODE_DEBUG_DIR`, `bench …` in src/debug.rs) in alternating
rounds on `firefox-ios/Client.xcodeproj` (~11,000 files, ~2,100 Swift files):

* cold open of the project (`--startup-bench`: first pixels on screen)
* the file scan, the tree, and when the navigator has the files
* a redraw with the project navigator, and with 5,000 search results
  (frames are gated on their work, the input's handling plus the draw,
  since a loaded machine's display ticks come at 30 Hz; the wall-clock
  times to the tick and to the frame on screen are reported beside them)
* Open Quickly: showing it, and each keystroke of a file name over every path
* Find in Workspace: a common identifier, a word with thousands of matches
  (the 5,000 cap), and a miss that reads every file
* memory (physical footprint and resident) idle after the open and after
  the search, and sourcekit-lsp's
* sourcekit-lsp ready and the first semantic tokens for a 5,000-line file
* typing in that file and in a 9,400-line one, a keystroke's work and its
  time to the next tick
* the index build: the app's scheme (Fennec) chosen in ⌃0's picker, its
  background build for the simulator zcode picks (`bench index`, as a save
  starts it) until it ends or `--index-cap` seconds, then sourcekit-lsp
  on a file the compilation database has and on the app's view
  controller, which it lacks (its semantic tokens, and where its
  arguments came from: zcode's build server); with each binary,
  reported, not gated. The rounds run without one (`ZCODE_INDEX_BUILD=0`)

Every gate has a generous absolute ceiling and, with a `master` build, a
relative one against master measured in the same run, so a loaded machine
(the load average is reported with each result) fails neither binary alone.
A master from before the benchmarks' control commands is measured through
`dump` (fewer gates get a relative limit).
`--only` keeps the gates whose names contain one of the texts. The report is
also written as JSON. `--self-test` checks the gate arithmetic and exits.
"""

import json
import os
import shutil
import signal
import statistics
import subprocess
import sys
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "scripts"))
import e2e  # noqa: E402  (the control channel's client; sets up a private ZCODE_CONFIG_DIR)

CACHE = os.path.expanduser(os.environ.get("ZCODE_BENCH_CACHE", "~/Library/Caches/zcode-bench"))
REPO = "https://github.com/mozilla-mobile/firefox-ios"
# firefox-ios main, 2026-10-17 ("Refactor MTE-6112 Xcode 27.1 Upgrade").
PIN = "0bd83b3b97a7dd97b23914030bd2292710b9fb5e"
CHECKOUT = os.path.join(CACHE, "firefox-ios")
PROJECT = os.path.join(CHECKOUT, "firefox-ios", "Client.xcodeproj")
LICENSE_HEAD = "Mozilla Public License, version 2.0"
# Typing: a 5,166-line view controller and the 9,427-line strings table.
# The index phase builds the app's scheme.
INDEX_SCHEME = "Fennec"
VIEW_CONTROLLER = "firefox-ios/Client/Frontend/Browser/BrowserViewController/Views/BrowserViewController.swift"
STRINGS = "firefox-ios/Shared/Strings.swift"
TYPED = "let measured = compute() + 1.25 * 3"
# Searches: ~250 matches in ~120 files; the 5,000 cap; nothing, every file read.
QUERIES = {"common": "viewDidLoad", "many": "the", "miss": "zzzzqqqxx"}

# (gate, unit, absolute ceiling, relative factor, relative slack): the branch's
# median must be within the ceiling and, with master, within
# master × factor + slack. Memory in MB, times in ms unless the name says s.
GATES = [
    ("open_pixels_ms", "ms", 400, 1.15, 25),
    ("scan_ms", "ms", 1000, 1.5, 50),
    ("tree_ms", "ms", 100, 1.5, 10),
    ("listed_at_ms", "ms", 1500, 1.3, 100),
    ("navigator_frame_ms", "ms", 33, 1.5, 4),
    ("open_quickly_show_ms", "ms", 50, 1.5, 5),
    ("open_quickly_key_ms", "ms", 16, 1.5, 3),
    ("open_quickly_match_ms", "ms", 10, 2.0, 1),
    ("find_common_ms", "ms", 2000, 1.5, 100),
    ("find_many_ms", "ms", 2000, 1.5, 100),
    ("find_miss_ms", "ms", 2000, 1.5, 100),
    ("find_frame_ms", "ms", 33, 1.5, 4),
    ("open_footprint_mb", "MB", 350, 1.3, 50),
    ("open_resident_mb", "MB", 400, 1.3, 50),
    ("search_footprint_mb", "MB", 700, 1.3, 60),
    ("search_resident_mb", "MB", 450, 1.3, 60),
    ("lsp_ready_s", "s", 60, 2.0, 5),
    ("tokens_s", "s", 120, 2.0, 5),
    ("typing_ms", "ms", 16, 1.5, 3),
    ("typing_big_ms", "ms", 16, 1.5, 3),
    ("typing_tick_ms", "ms", 50, 1.5, 5),
    ("typing_big_tick_ms", "ms", 50, 1.5, 5),
    ("lsp_footprint_mb", "MB", 2000, None, None),
]
# Reported beside the gates, never gated.
REPORTED = ["files", "find_common_results", "find_many_results", "typing_drawn_ms", "typing_big_drawn_ms", "open_quickly_key_drawn_ms", "navigator_frame_drawn_ms", "find_frame_drawn_ms", "legacy"]


def arg(name, default=None):
    return sys.argv[sys.argv.index(name) + 1] if name in sys.argv else default


def load():
    return [round(l, 1) for l in os.getloadavg()]


def run(cmd, **kw):
    return subprocess.run(cmd, check=True, capture_output=True, text=True, **kw)


# ---------------------------------------------------------------------------
# The project and the binaries


def fetch_project():
    """firefox-ios at the pin, shallow, in the cache; verified, never written."""
    if "--project" in sys.argv:
        return os.path.abspath(arg("--project"))
    head = None
    if os.path.isdir(os.path.join(CHECKOUT, ".git")):
        head = subprocess.run(["git", "-C", CHECKOUT, "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
    if head != PIN:
        print(f"  fetching firefox-ios at {PIN[:7]} into {CHECKOUT} (shallow)")
        os.makedirs(CHECKOUT, exist_ok=True)
        if head is None:
            run(["git", "init", "-q", CHECKOUT])
            run(["git", "-C", CHECKOUT, "remote", "add", "origin", REPO])
        run(["git", "-C", CHECKOUT, "fetch", "-q", "--depth", "1", "origin", PIN])
        run(["git", "-C", CHECKOUT, "checkout", "-q", "--force", PIN])
    with open(os.path.join(CHECKOUT, "LICENSE")) as f:
        assert f.readline().strip() == LICENSE_HEAD, "firefox-ios's license changed; check it before benchmarking on it"
    assert_clean(CHECKOUT)
    return PROJECT


def assert_clean(checkout):
    status = subprocess.run(["git", "-C", checkout, "status", "--porcelain"], capture_output=True, text=True).stdout
    assert not status.strip(), f"the benchmark project has changes:\n{status}"


def master_binary():
    """A release build of `master` in the cache, keyed by its commit."""
    if "--no-master" in sys.argv:
        return None
    if "--master" in sys.argv:
        return os.path.abspath(arg("--master"))
    sha = run(["git", "-C", ROOT, "rev-parse", "master"]).stdout.strip()
    head = run(["git", "-C", ROOT, "rev-parse", "HEAD"]).stdout.strip()
    if sha == head and not run(["git", "-C", ROOT, "status", "--porcelain"]).stdout.strip():
        print("  master is HEAD: the relative gates compare two builds of the same commit")
    binary = os.path.join(CACHE, "builds", sha, "zcode")
    if not os.path.exists(binary):
        src = os.path.join(CACHE, "builds", sha, "src")
        print(f"  building master ({sha[:7]}) in {os.path.dirname(binary)} (minutes, once per commit)")
        shutil.rmtree(src, ignore_errors=True)
        os.makedirs(src)
        archive = subprocess.Popen(["git", "-C", ROOT, "archive", "--format=tar", sha], stdout=subprocess.PIPE)
        run(["tar", "-x", "-C", src], stdin=archive.stdout)
        assert archive.wait() == 0, "git archive master"
        target = os.path.join(CACHE, "builds", "target")
        env = dict(os.environ, CARGO_TARGET_DIR=target)
        subprocess.run(["cargo", "build", "--release", "--manifest-path", os.path.join(src, "Cargo.toml")], check=True, env=env)
        shutil.copy2(os.path.join(target, "release", "zcode"), binary)
        shutil.rmtree(src, ignore_errors=True)
    return binary


# ---------------------------------------------------------------------------
# Measuring


def launch_once(binary, project):
    """One `--startup-bench` launch of the project: first pixels, in ms."""
    env = dict(os.environ, ZCODE_NEW_INSTANCE="1", ZCODE_INDEX_BUILD="0")
    out = run([binary, "--startup-bench", project], env=env).stdout
    return json.loads(out.strip().splitlines()[-1])["total_to_first_pixels_ms"]


def idle_memory(status, seconds):
    """The lowest footprint and resident size over `seconds` of idling (the
    sampler runs every 2 s): what zcode holds, under the compositor's
    transient attribution while the window redraws."""
    footprint, resident = [], []
    deadline = time.time() + seconds
    while time.time() < deadline:
        time.sleep(2)
        m = status()["memory"]
        footprint.append(m["footprint_mb"])
        resident.append(m["resident_mb"])
    lowest = lambda values: min(v for v in values if v is not None) if any(v is not None for v in values) else None
    return lowest(footprint), lowest(resident)


class Status:
    """`bench status`, or what `dump` offers from a zcode built before it
    (a `master` from before the benchmarks): the same keys, None where the
    older build can't say."""

    def __init__(self, z):
        self.z = z
        try:
            z.run("bench status")
            self.legacy = False
        except RuntimeError:
            self.legacy = True

    def __call__(self):
        if not self.legacy:
            return json.loads(self.z.run("bench status")[-1])
        s = self.z.dump()
        active = s["active"]
        return {
            "files": s["files"],
            "timings": {},
            "memory": {
                "footprint_mb": s["memory"]["footprint_mb"],
                "resident_mb": None,
                "language_server_mb": s["memory"]["language_server_mb"],
            },
            "lsp": s["lsp"],
            "lsp_progress": s["lsp_progress"],
            "scheme": s["scheme"],
            "indexing": s["indexing"],
            "compile_database": None,
            "find_results": s["find_results"],
            "find_files": None,
            "find_searching": False,
            "active": active and {
                "path": active["path"],
                "dirty": active["dirty"],
                "semantic_tokens": s["language_features"]["semantic_tokens"]["count"] if s["language_features"]["semantic_tokens"] else 0,
            },
            "navigator": s["navigator"],
            "modal_open": s["modal_open"],
        }


def frames(z, n=5):
    """A forced redraw's work and its time to on screen, medians of `n`."""
    samples = [json.loads(z.run("bench frame")[-1]) for _ in range(n)]
    return statistics.median(f["work_ms"] for f in samples), statistics.median(f["drawn_ms"] for f in samples)


def timed(z, command):
    """A `bench …` input's frame, or None from a build without the command."""
    try:
        return json.loads(z.run(command)[-1])
    except RuntimeError:
        return None


def undo_all(z, status):
    for _ in range(100):
        s = status()
        if not s["active"] or not s["active"]["dirty"]:
            return
        z.run("keys cmd-z")
    raise AssertionError("still dirty after 100 undos")


class IndexBuildStopper:
    """Stops the index build of a zcode built before `ZCODE_INDEX_BUILD`
    (an older `master`): its `xcodebuild` of the whole app would load the
    machine through the session. Only that zcode's own children are
    touched; zcode tries the index build once per open, so it isn't
    started again."""

    def __init__(self, pid):
        self.pid = pid
        self.stopped = []
        self.done = threading.Event()
        self.thread = threading.Thread(target=self.watch, daemon=True)
        self.thread.start()

    def watch(self):
        while not self.done.wait(0.5):
            # The index build's xcodebuild (exec'd by nice) builds into its own
            # derived data; the scheme listing and build settings don't.
            children = subprocess.run(["pgrep", "-P", str(self.pid), "-f", "xcodebuild.*IndexDerivedData"], capture_output=True, text=True).stdout.split()
            for child in map(int, children):
                if child not in self.stopped:
                    self.stopped.append(child)
                    try:
                        os.kill(child, signal.SIGINT)
                    except ProcessLookupError:
                        pass

    def stop(self):
        self.done.set()
        self.thread.join()


def session(binary, project, index_cap=None):
    """One zcode on the project, every measurement but the cold open. With
    `index_cap`, the index build is left to run and waited for."""
    # No index build on open: the rounds measure without one, and the index
    # phase starts its own for the app's scheme.
    z = e2e.Zcode(project, {"ZCODE_INDEX_BUILD": "0"}, binary=binary)
    status = Status(z)
    stopper = IndexBuildStopper(z.proc.pid) if status.legacy else None
    timing = lambda key, field="ms": (status()["timings"].get(key) or {}).get(field)
    r = {"load": load(), "legacy": status.legacy}
    try:
        started = time.time()
        for _ in range(600):
            s = status()
            if s["files"] > 0 and ("tree" in s["timings"] or status.legacy):
                break
            time.sleep(0.2)
        else:
            raise TimeoutError(f"no files after {time.time() - started:.0f} s")
        r["files"] = s["files"]
        # zcode keeps a project's compilation database in ~/Library/Caches/zcode
        # whatever its configuration folder; the index phase removes the one
        # it writes, so the rounds open the project without one.
        r["compile_database_at_open"] = bool(s["compile_database"])
        if r["compile_database_at_open"]:
            print(f"    warning: {s['compile_database']} was there at open (an interrupted index phase?); "
                  "sourcekit-lsp's numbers are with it", flush=True)
        r["scan_ms"] = timing("scan")
        r["tree_ms"] = timing("tree")
        r["listed_at_ms"] = timing("tree", "at_ms")
        r["open_footprint_mb"], r["open_resident_mb"] = idle_memory(status, 12)
        r["navigator_frame_ms"], r["navigator_frame_drawn_ms"] = (None, None) if status.legacy else frames(z)

        shown = timed(z, "bench action zcode::OpenQuickly")
        if shown is None:
            z.run("keys cmd-shift-o", "wait 300")
        r["open_quickly_show_ms"] = shown and shown["work_ms"]
        typed = json.loads(z.run("type-latency BrowserViewController", timeout=240)[-1])
        r["open_quickly_key_ms"] = typed.get("work_median_ms")
        r["open_quickly_key_drawn_ms"] = typed.get("drawn_median_ms")
        r["open_quickly_match_ms"] = timing("open_quickly")
        z.run("keys escape", "wait 200")

        for name, query in QUERIES.items():
            z.run("keys cmd-shift-f", "wait 200", "keys cmd-a", f"type {query}", "keys enter")
            for _ in range(1200):
                s = status()
                if not s["find_searching"] and ("find" in s["timings"] or status.legacy):
                    break
                time.sleep(0.05)
            else:
                raise TimeoutError(f"search {query!r}")
            if status.legacy:
                # No searching flag there: give the search its time.
                time.sleep(3)
                s = status()
            r[f"find_{name}_ms"] = timing("find")
            r[f"find_{name}_results"] = s["find_results"]
            if name == "many":
                r["find_frame_ms"], r["find_frame_drawn_ms"] = (None, None) if status.legacy else frames(z)
                r["search_footprint_mb"], r["search_resident_mb"] = idle_memory(status, 12)
        assert r["find_many_results"] >= 5000 and r["find_miss_results"] == 0, (r["find_many_results"], r["find_miss_results"])
        z.run("keys cmd-1", "wait 300")

        # The language server starts with the first Swift file.
        big = os.path.join(os.path.dirname(os.path.dirname(project)), VIEW_CONTROLLER)
        opened = time.time()
        z.run(f"open {big}:2500")
        ready = tokens = None
        while time.time() - opened < 180 and tokens is None:
            s = status()
            if ready is None and s["lsp"] == "Ready":
                ready = time.time() - opened
            if s["active"] and (s["active"]["semantic_tokens"] or 0) > 0:
                tokens = time.time() - opened
            time.sleep(0.25)
        r["lsp_ready_s"] = ready
        r["tokens_s"] = tokens
        r["lsp_status"] = s["lsp"]
        z.run("keys end", "keys enter")
        typed = json.loads(z.run(f"type-latency {TYPED}", timeout=240)[-1])
        r["typing_ms"], r["typing_tick_ms"], r["typing_drawn_ms"] = typed.get("work_median_ms"), typed["median_ms"], typed.get("drawn_median_ms")
        undo_all(z, status)
        z.run("keys cmd-w", "wait 300")

        strings = os.path.join(os.path.dirname(os.path.dirname(project)), STRINGS)
        z.run(f"open {strings}:5000", "wait 1500", "keys end", "keys enter")
        typed = json.loads(z.run(f"type-latency {TYPED}", timeout=240)[-1])
        r["typing_big_ms"], r["typing_big_tick_ms"], r["typing_big_drawn_ms"] = typed.get("work_median_ms"), typed["median_ms"], typed.get("drawn_median_ms")
        undo_all(z, status)
        z.run("keys cmd-w", "wait 300")
        r["lsp_footprint_mb"] = status()["memory"]["language_server_mb"]

        if index_cap:
            # The app's scheme, chosen as in ⌃0's picker (zcode opens with
            # the first scheme, a macOS library here), then its build for the
            # destination zcode picks (an iPhone simulator).
            z.run("keys ctrl-0", "wait 400", f"type {INDEX_SCHEME}", "wait 300", "keys enter")
            chosen = time.time()
            while time.time() - chosen < 120:
                s = status()
                if s["scheme"] == INDEX_SCHEME and s["destination"] and not s["destination"].startswith("My Mac"):
                    break
                time.sleep(1)
            else:
                raise TimeoutError(f"scheme {INDEX_SCHEME} with a simulator: {s['scheme']}, {s['destination']}")
            r["index_scheme"], r["index_destination"] = s["scheme"], s["destination"]
            # A first build, every time: without a database zcode clears an
            # earlier build's products (src/build/compiledb.rs).
            if s["compile_database"]:
                os.remove(s["compile_database"])
            z.run("bench index")
            started = time.time()
            began = None
            s = status()
            while time.time() - started < index_cap:
                s = status()
                if s["indexing"] and began is None:
                    began = time.time() - started
                if began is not None and not s["indexing"]:
                    break
                time.sleep(1)
            r["index_started_s"] = began and round(began, 1)
            r["index_s"] = round(time.time() - started, 1)
            r["index_done"] = began is not None and not s["indexing"]
            r["compile_database"] = bool(s["compile_database"])
            database = s["compile_database"]
            if database:
                with open(database) as f:
                    commands = json.load(f)
                r["compile_database_files"] = len(commands)
                r["compile_database_has_client"] = any(c["file"].endswith(VIEW_CONTROLLER) for c in commands)
            if r["index_done"] and r["compile_database"]:
                # What sourcekit-lsp does with the database: restarted with
                # it, ready again, and the tokens of a file the database
                # covers (the strings table, in the Shared framework).
                z.run(f"open {strings}:5000")
                again = time.time()
                while time.time() - again < 120:
                    s = status()
                    if s["lsp"] == "Ready" and s["active"] and (s["active"]["semantic_tokens"] or 0) > 0:
                        break
                    time.sleep(0.5)
                r["lsp_ready_with_database_s"] = round(time.time() - again, 1)
                r["lsp_footprint_with_database_mb"] = status()["memory"]["language_server_mb"]
            if r["index_done"]:
                # The app's view controller, which the database lacks (the app
                # doesn't compile without bootstrap.sh). zcode's build server
                # gives it the app target's arguments from its build settings;
                # with the database alone sourcekit-lsp gave it no tokens.
                z.run(f"open {big}:2500")
                again = time.time()
                r["app_tokens_after_index_s"] = None
                while time.time() - again < 120:
                    s = status()
                    if s["lsp"] == "Ready" and s["active"] and (s["active"]["semantic_tokens"] or 0) > 0:
                        r["app_tokens_after_index_s"] = round(time.time() - again, 1)
                        break
                    time.sleep(0.5)
                # More once its imports resolve: let them settle.
                time.sleep(5)
                r["app_tokens_after_index"] = (status()["active"] or {}).get("semantic_tokens")
                arguments = timed(z, f"bsp options {big}")
                r["app_arguments"] = arguments.get("origin") if arguments else None
                r["build_server"] = s.get("build_server")
            if database:
                os.remove(database)
    finally:
        if stopper:
            stopper.stop()
            r["index_builds_stopped"] = len(stopper.stopped)
        z.quit()
    r["load_after"] = load()
    return r


# ---------------------------------------------------------------------------
# Gates


def medians(rounds):
    """Per-metric medians over a binary's rounds (None when a round lacks it)."""
    out = {}
    for key in {k for r in rounds for k in r}:
        values = [r[key] for r in rounds if isinstance(r.get(key), (int, float))]
        if values:
            out[key] = statistics.median(values)
    return out


def evaluate(gate, branch, master):
    """Whether a gate holds, and why not.

    >>> evaluate(("x_ms", "ms", 100, 1.5, 10), 50, None)
    (True, '')
    >>> evaluate(("x_ms", "ms", 100, 1.5, 10), 120, None)
    (False, 'over the 100 ms ceiling')
    >>> evaluate(("x_ms", "ms", 100, 1.5, 10), 50, 20)
    (False, 'master 20 ms: over 20 × 1.5 + 10 = 40 ms')
    >>> evaluate(("x_ms", "ms", 100, 1.5, 10), 40, 20)
    (True, '')
    >>> evaluate(("x_mb", "MB", 100, None, None), 90, 10)
    (True, '')
    >>> evaluate(("x_ms", "ms", 100, 1.5, 10), None, 20)
    (False, 'not measured')
    """
    name, unit, ceiling, factor, slack = gate
    if branch is None:
        return False, "not measured"
    if branch > ceiling:
        return False, f"over the {ceiling:g} {unit} ceiling"
    if master is not None and factor is not None:
        limit = master * factor + slack
        if branch > limit:
            return False, f"master {master:g} {unit}: over {master:g} × {factor:g} + {slack:g} = {limit:g} {unit}"
    return True, ""


def fmt(value, unit):
    if value is None:
        return "—"
    return f"{value:.0f} {unit}" if value >= 100 else f"{value:.1f} {unit}"


def self_test():
    import doctest

    failed, _ = doctest.testmod(sys.modules[__name__])
    assert medians([{"a": 1, "b": None}, {"a": 3}, {"a": 2}]) == {"a": 2}
    print("bench.py self-test ok" if not failed else f"{failed} failed")
    sys.exit(1 if failed else 0)


# ---------------------------------------------------------------------------


def main():
    if "--self-test" in sys.argv:
        self_test()
    if not os.path.exists(e2e.BINARY):
        sys.exit("build first: cargo build --release")
    rounds = int(arg("--rounds", "3"))
    only = e2e.ONLY
    print(f"Scale benchmarks (load average {load()}):")
    project = fetch_project()
    master = master_binary()
    binaries = [("branch", e2e.BINARY)] + ([("master", master)] if master else [])
    results = {label: [] for label, _ in binaries}
    opens = {label: [] for label, _ in binaries}

    if "--skip-open" not in sys.argv:
        print(f"  cold open of {os.path.basename(project)} ({rounds + 2} launches each, alternating, the first a warm-up):")
        for label, binary in binaries:
            launch_once(binary, project)
        for i in range(rounds + 1):
            for label, binary in binaries if i % 2 == 0 else reversed(binaries):
                opens[label].append(launch_once(binary, project))
        for label in opens:
            print(f"    {label}: first pixels {', '.join(f'{v:.0f}' for v in opens[label])} ms")
    for i in range(rounds):
        for label, binary in binaries if i % 2 == 0 else reversed(binaries):
            print(f"  round {i + 1}, {label} (load average {load()})…", flush=True)
            try:
                r = session(binary, project)
            except Exception as e:  # noqa: BLE001  (a session that fails leaves its gates unmeasured)
                r = {"load": load(), "error": f"{type(e).__name__}: {e}"}
            results[label].append(r)
            print(f"    {json.dumps({k: (round(v, 1) if isinstance(v, float) else v) for k, v in r.items()})}")
    index = {}
    if "--skip-index" not in sys.argv:
        cap = int(arg("--index-cap", "600"))
        for label, binary in binaries:
            print(f"  the index build with the {label} binary (up to {cap} s)…", flush=True)
            try:
                i = index[label] = session(binary, project, index_cap=cap)
            except Exception as e:  # noqa: BLE001
                index[label] = {"error": f"{type(e).__name__}: {e}"}
                print(f"    failed: {index[label]['error']}")
                continue
            if i["index_started_s"] is None:
                print(f"    {i['index_scheme']} on {i['index_destination']}: the index build hadn't started after {i['index_s']} s")
            else:
                print(f"    {i['index_scheme']} on {i['index_destination']}: started after {i['index_started_s']} s, "
                      f"{'finished' if i['index_done'] else 'still running'} after {i['index_s']} s; "
                      f"compilation database {'written' if i['compile_database'] else 'not written'}"
                      + (f" ({i['compile_database_files']} files, {'with' if i['compile_database_has_client'] else 'without'} the app's)" if i["compile_database"] else "")
                      + (f"; sourcekit-lsp ready with it in {i['lsp_ready_with_database_s']} s" if "lsp_ready_with_database_s" in i else ""))
            if "app_tokens_after_index" in i:
                print(f"    the app's view controller (not in the database): "
                      + (f"semantic tokens after {i['app_tokens_after_index_s']} s, {i['app_tokens_after_index']} of them" if i["app_tokens_after_index_s"] is not None else "no semantic tokens in 120 s")
                      + (f"; arguments from {i['app_arguments']}" if i.get("app_arguments") else ""))
    if rounds == 0:
        print("  no rounds: nothing gated")
        sys.exit(0)
    assert_clean(CHECKOUT) if project == PROJECT else None

    summary = {label: medians(rs) for label, rs in results.items()}
    for label in opens:
        if opens[label]:
            summary[label]["open_pixels_ms"] = statistics.median(opens[label])
    branch, master_values = summary["branch"], summary.get("master", {})
    failed = []
    print(f"  gates (medians of {rounds}; load average {load()}):")
    for gate in GATES:
        name, unit = gate[0], gate[1]
        if only and not any(part in name for part in only):
            continue
        if name == "open_pixels_ms" and "--skip-open" in sys.argv:
            continue
        ok, why = evaluate(gate, branch.get(name), master_values.get(name))
        mark = "✓" if ok else "✗"
        against = f"   master {fmt(master_values.get(name), unit)}" if master else ""
        print(f"    {mark} {name:<24} {fmt(branch.get(name), unit):>10}{against}{'   ' + why if why else ''}")
        if not ok:
            failed.append(name)
    for name in REPORTED:
        shown = lambda v: fmt(v, "ms") if name.endswith("_ms") else str(v)
        print(f"      {name:<24} {shown(branch.get(name)):>10}" + (f"   master {shown(master_values.get(name))}" if master else ""))

    report = {
        "project": project,
        "pin": PIN,
        "rounds": rounds,
        "load": load(),
        "opens": opens,
        "sessions": results,
        "index": index,
        "medians": summary,
        "failed": failed,
    }
    path = arg("--report") or os.path.join(CACHE, "reports", time.strftime("%Y-%m-%d-%H%M%S") + ".json")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        json.dump(report, f, indent=1)
    print(f"  report: {path}")
    print(f"\n{len(failed)} gate(s) failed" if failed else "\nall gates passed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

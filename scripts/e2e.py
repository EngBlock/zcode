#!/usr/bin/env python3
"""End-to-end UI checks for zcode, driven through its debug control channel.

    cargo build --release && python3 scripts/e2e.py [--simulator]

Launches zcode on the sample projects with ZCODE_DEBUG_DIR set, sends
keystrokes/actions, and asserts on the JSON state from `dump`. `--simulator`
also builds, runs and debugs the iOS sample in a simulator (slower).
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SAMPLES = os.path.normpath(os.path.join(ROOT, "samples"))
# Tests run against a private copy so edits, breakpoints and session state from
# other work in the repo can't leak in (and test edits never touch the repo).
WORK = tempfile.mkdtemp(prefix="zcode-e2e-samples-")
BINARY = os.path.join(ROOT, "target", "release", "zcode")
DEBUG_DIR = tempfile.mkdtemp(prefix="zcode-e2e-")

passed, failed = 0, 0


class Zcode:
    def __init__(self, project):
        self.log_offset = 0
        env = dict(os.environ, ZCODE_DEBUG_DIR=DEBUG_DIR)
        self.proc = subprocess.Popen([BINARY, project], env=env,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.run("wait 1")

    def run(self, *commands, timeout=90):
        log_path = os.path.join(DEBUG_DIR, "log")
        before = self._count(log_path)
        tmp = os.path.join(DEBUG_DIR, "commands.tmp")
        with open(tmp, "w") as f:
            f.write("\n".join(commands) + "\n")
        os.rename(tmp, os.path.join(DEBUG_DIR, "commands"))
        deadline = time.time() + timeout
        while time.time() < deadline:
            if self.proc.poll() is not None:
                raise RuntimeError("zcode exited")
            if self._count(log_path) - before >= len(commands):
                break
            time.sleep(0.1)
        else:
            raise TimeoutError(commands)
        with open(log_path) as f:
            lines = f.read().splitlines()
        new = lines[self.log_offset:]
        self.log_offset = len(lines)
        errors = [l for l in new if l.startswith("err ")]
        if errors:
            raise RuntimeError(errors)
        return [l for l in new if l.startswith("{")]

    def dump(self):
        return json.loads(self.run("dump")[-1])

    def wait_for(self, predicate, timeout=120, interval=1.0):
        deadline = time.time() + timeout
        while time.time() < deadline:
            state = self.dump()
            if predicate(state):
                return state
            time.sleep(interval)
        raise TimeoutError("condition not met; last state: " + json.dumps(state)[:600])

    @staticmethod
    def _count(path):
        try:
            with open(path) as f:
                return sum(1 for l in f if l.startswith(("ok ", "err ")))
        except FileNotFoundError:
            return 0

    def quit(self):
        try:
            self.run("action zcode::Quit", timeout=5)
        except Exception:
            pass
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def check(name, fn):
    global passed, failed
    try:
        fn()
        passed += 1
        print(f"  ✓ {name}")
    except Exception as e:  # noqa: BLE001
        failed += 1
        print(f"  ✗ {name}: {e}")


def keys_right(n):
    return "keys " + " ".join(["right"] * n)


def line_of(path, needle):
    """1-based line number of the first line containing `needle`."""
    with open(path) as f:
        for number, line in enumerate(f, 1):
            if needle in line:
                return number
    raise AssertionError(f"{needle!r} not found in {path}")


def column_of(path, needle, offset=0):
    """Rightward moves from the start of the line to `needle` (+ offset)."""
    with open(path) as f:
        line = f.read().splitlines()[line_of(path, needle) - 1]
    return line.index(needle) + offset


def xcode_project_checks(z, samples, sim):
    hello = os.path.join(samples, "HelloZ", "HelloZ")
    content, counter = os.path.join(hello, "ContentView.swift"), os.path.join(hello, "Counter.swift")
    counter_line = line_of(content, "Counter()")
    tap_line = line_of(content, 'print("tap')
    appear_line = line_of(content, 'print("HelloZ launched")')
    count_line = line_of(counter, "count += amount")
    decl_line = line_of(counter, "private(set) var count")

    def lsp_ready():
        z.run(f"open {content}")
        # A never-indexed project runs a background index build that restarts
        # sourcekit-lsp when it finishes; wait for that too.
        z.wait_for(lambda s: s["lsp"] == "Ready" and not s["issues"] and not s["indexing"], timeout=300)

    def open_quickly():
        z.run("keys cmd-shift-o", "type cou", "wait 300")
        assert z.dump()["modal_open"]
        z.run("keys enter", "wait 300")
        s = z.dump()
        assert s["active"]["path"].endswith("Counter.swift"), s["active"]["path"]

    def definition():
        # Right after sourcekit-lsp restarts with the compile database its
        # first answers can be empty; retry like a user pressing ⌃⌘J again.
        for _ in range(6):
            z.run(f"open {content}:{counter_line}", "wait 300", keys_right(column_of(content, "Counter()", 2)), "keys ctrl-cmd-j")
            try:
                s = z.wait_for(lambda s: s["active"]["path"].endswith("Counter.swift"), timeout=5, interval=0.5)
                break
            except TimeoutError:
                continue
        assert s["active"]["path"].endswith("Counter.swift"), s["active"]["path"]
        assert s["active"]["line"] == line_of(counter, "struct Counter"), s["active"]

    def completion():
        z.run(f"open {content}:{tap_line}", "wait 1500", "keys end", "keys enter", "type counter.inc")
        s = z.wait_for(lambda s: s["active"]["completion_menu_open"], timeout=15, interval=0.5)
        assert any("increment" in i for i in s["active"]["completion_items"]), s["active"]
        z.run("keys down", "keys enter", "keys tab", "type 7")
        assert "increment(by: 7)" in z.dump()["active"]["line_text"]
        z.run("keys cmd-z cmd-z cmd-z cmd-z cmd-z cmd-z cmd-z cmd-z")

    def comment_and_undo():
        z.run(f"open {counter}:{count_line}", "wait 300", "keys cmd-/")
        assert z.dump()["active"]["line_text"].strip().startswith("// ")
        z.run("keys cmd-z")
        s = z.dump()
        assert not s["active"]["dirty"], "undo should restore the saved state"

    def document_items():
        z.run(f"open {counter}", "keys ctrl-6", "wait 1500", "type reset", "keys enter", "wait 300")
        assert z.dump()["active"]["selection"] == "reset()"

    def find_and_references():
        z.run("keys cmd-shift-f", "wait 200", "type increment", "keys enter", "wait 800")
        assert z.dump()["find_results"] >= 2
        z.run(f"open {counter}:{decl_line}", "wait 300", keys_right(column_of(counter, "count =", 2)),
              "keys ctrl-shift-cmd-f", "wait 2500")
        assert z.dump()["find_results"] >= 3

    def build_error_and_issue_navigation():
        z.run(f"open {counter}:{count_line}", "wait 300", "keys end", "type  + oops", "keys cmd-b")
        s = z.wait_for(lambda s: s["phase"].startswith("Finished"), timeout=180)
        assert "success: false" in s["phase"], s["phase"]
        assert any("oops" in i for i in s["issues"]), s["issues"]
        assert s["navigator"] == "Issues"
        z.run("keys cmd-'", "wait 300")
        assert z.dump()["active"]["path"].endswith("Counter.swift")
        z.run("keys cmd-z", "keys cmd-s")

    def unit_tests_on_simulator():
        z.run("keys cmd-u")
        s = z.wait_for(lambda s: s["phase"].startswith("Finished"), timeout=300)
        assert "success: true" in s["phase"], s["phase"]
        assert sorted(s["tests"]) == ["Passed incrementAddsOne()", "Passed resetClearsCount()"], s["tests"]

    def run_on_simulator():
        z.run("keys cmd-r")
        z.wait_for(lambda s: any("HelloZ launched" in l for l in s["console_tail"]), timeout=300)
        z.run("keys cmd-.", "wait 1500")
        phase = z.dump()["phase"]
        assert phase == "Stopped" or phase.startswith("Finished"), phase

    def debug_on_simulator():
        z.run(f"open {content}:{appear_line}", "wait 300", "keys cmd-\\", "keys cmd-r")
        s = z.wait_for(lambda s: s["debugger"]["state"].startswith("Stopped") and s["debugger"]["frames"], timeout=300)
        assert f"ContentView.swift:{appear_line}" in s["debugger"]["frames"][0], s["debugger"]
        z.run(f"open {content}:{appear_line}", "keys cmd-\\", "keys cmd-.", "wait 1500")

    check("sourcekit-lsp ready with compile database (no false errors)", lsp_ready)
    check("Open Quickly (⇧⌘O)", open_quickly)
    check("Jump to Definition across files (⌃⌘J)", definition)
    check("Completion + placeholder Tab", completion)
    check("Comment (⌘/) and undo restores clean state", comment_and_undo)
    check("Document Items (⌃6)", document_items)
    check("Find in Workspace (⇧⌘F) and references (⌃⇧⌘F)", find_and_references)
    check("Build error → Issue navigator → next issue (⌘')", build_error_and_issue_navigation)
    if sim:
        check("Unit tests in iOS Simulator (⌘U)", unit_tests_on_simulator)
        check("Run on iOS Simulator (⌘R) with console", run_on_simulator)
        check("Debug on iOS Simulator (breakpoint stop)", debug_on_simulator)


def package_checks(z, samples):
    kit = os.path.join(samples, "ZKit")
    main = os.path.join(kit, "Sources", "zhello", "main.swift")
    loop_line = line_of(main, "for i in")

    def run_package():
        z.run("keys cmd-r")
        s = z.wait_for(lambda s: s["phase"].startswith("Finished"), timeout=300)
        assert "Hello, zcode!" in s["console_tail"], s["console_tail"]

    def test_package():
        z.run("keys cmd-u")
        s = z.wait_for(lambda s: s["phase"].startswith("Finished"), timeout=300)
        assert s["tests"] and all(t.startswith("Passed") for t in s["tests"]), s["tests"]

    def debug_package():
        z.run(f"open {main}:{loop_line}", "wait 300", "keys cmd-\\", "keys cmd-r")
        s = z.wait_for(
            lambda s: s["debugger"]["state"].startswith("Stopped") and s["debugger"]["variables"],
            timeout=300,
            interval=0.5,
        )
        assert any(v.startswith("i = ") for v in s["debugger"]["variables"]), s["debugger"]
        z.run("keys f6")
        z.wait_for(lambda s: s["debugger"]["frames"] and f"main.swift:{loop_line + 1}" in s["debugger"]["frames"][0],
                   timeout=20, interval=0.5)
        z.run(f"open {main}:{loop_line}", "keys cmd-\\", "keys ctrl-cmd-y")
        z.wait_for(lambda s: s["phase"].startswith("Finished"), timeout=60)

    check("Swift package: Run on My Mac (⌘R)", run_package)
    check("Swift package: Test (⌘U) → Test navigator", test_package)
    check("Swift package: breakpoint, variables, step over (F6)", debug_package)


def warm_open(samples):
    """Starts a regular (single-instance) zcode, then times `zcode <project>`."""
    socket = os.path.expanduser("~/Library/Caches/zcode/zcode.sock")
    log = os.path.join(WORK, "instance.log")
    env = {k: v for k, v in os.environ.items() if k not in ("ZCODE_DEBUG_DIR",)}
    env["ZCODE_TRACE_STARTUP"] = "1"
    with open(log, "w") as out:
        server = subprocess.Popen([BINARY], env=env, stdout=out, stderr=out)
    try:
        deadline = time.time() + 15
        while not os.path.exists(socket) and time.time() < deadline:
            time.sleep(0.1)
        time.sleep(1.0)
        started = time.time()
        subprocess.run([BINARY, os.path.join(samples, "ZKit")], env=env, check=True, timeout=10)
        client_ms = (time.time() - started) * 1000
        deadline = time.time() + 10
        frame_ms = None
        while time.time() < deadline and frame_ms is None:
            for line in open(log):
                if line.startswith("[instance] first frame in "):
                    value = line.split("in ", 1)[1].strip()
                    frame_ms = float(value[:-2]) if value.endswith("ms") else float(value[:-1]) * 1000
            time.sleep(0.1)
        print(f"    client returned in {client_ms:.0f} ms; new window's first frame after {frame_ms} ms")
        assert frame_ms is not None and frame_ms < 150, frame_ms
    finally:
        server.kill()
        server.wait()
        if os.path.exists(socket):
            os.remove(socket)


def main():
    if not os.path.exists(BINARY):
        sys.exit("build first: cargo build --release")
    sim = "--simulator" in sys.argv
    print("Startup benchmark (warm; the first launch after a rebuild includes macOS's binary scan):")
    for _ in range(2):
        out = subprocess.run([BINARY, "--startup-bench"], capture_output=True, text=True).stdout.strip()
    print("  " + out.splitlines()[-1] if out else "  (no output)")

    samples = os.path.join(WORK, "samples")
    warm_open_check_pending = True
    subprocess.run(["git", "-C", ROOT, "archive", "--format=tar", "-o", os.path.join(WORK, "samples.tar"), "HEAD", "samples"], check=True)
    subprocess.run(["tar", "-xf", "samples.tar"], cwd=WORK, check=True)
    samples = os.path.realpath(samples)

    print("Xcode project (samples/HelloZ):")
    z = Zcode(os.path.join(samples, "HelloZ"))
    try:
        xcode_project_checks(z, samples, sim)
    finally:
        z.quit()

    if warm_open_check_pending:
        print("Single instance:")
        check("Warm open through a running instance (< 150 ms to first frame)", lambda: warm_open(samples))

    print("Swift package (samples/ZKit):")
    z = Zcode(os.path.join(samples, "ZKit"))
    try:
        package_checks(z, samples)
    finally:
        z.quit()

    shutil.rmtree(DEBUG_DIR, ignore_errors=True)
    shutil.rmtree(WORK, ignore_errors=True)
    print(f"\n{passed} passed, {failed} failed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

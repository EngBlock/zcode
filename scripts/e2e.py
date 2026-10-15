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


def xcode_project_checks(z, sim):
    hello = os.path.join(SAMPLES, "HelloZ", "HelloZ")
    content, counter = os.path.join(hello, "ContentView.swift"), os.path.join(hello, "Counter.swift")
    originals = {p: open(p).read() for p in (content, counter)}

    def lsp_ready():
        z.run(f"open {content}")
        # On a never-indexed project the background index build restarts
        # sourcekit-lsp when it finishes; wait for that too.
        z.wait_for(lambda s: s["lsp"] == "Ready" and not s["issues"] and not s["indexing"], timeout=300)

    def open_quickly():
        z.run("keys cmd-shift-o", "type cou", "wait 300")
        assert z.dump()["modal_open"]
        z.run("keys enter", "wait 300")
        s = z.dump()
        assert s["active"]["path"].endswith("Counter.swift"), s["active"]["path"]

    def definition():
        z.run(f"open {content}:4", "wait 300", "keys end left left left", "keys ctrl-cmd-j")
        s = z.wait_for(lambda s: s["active"]["path"].endswith("Counter.swift"), timeout=20, interval=0.5)
        assert s["active"]["line"] == 4, s["active"]

    def completion():
        z.run(f"open {content}:16", "wait 1500", "keys end", "keys enter", "type counter.inc")
        s = z.wait_for(lambda s: s["active"]["completion_menu_open"], timeout=15, interval=0.5)
        assert any("increment" in i for i in s["active"]["completion_items"]), s["active"]
        z.run("keys down", "keys enter", "keys tab", "type 7")
        assert "increment(by: 7)" in z.dump()["active"]["line_text"]
        z.run("keys cmd-z cmd-z cmd-z cmd-z cmd-z cmd-z cmd-z cmd-z")

    def comment_and_undo():
        z.run(f"open {counter}:8", "wait 300", "keys cmd-/")
        assert z.dump()["active"]["line_text"].strip().startswith("// ")
        z.run("keys cmd-z")
        s = z.dump()
        assert not s["active"]["dirty"], "undo should restore the saved state"

    def cut_and_paste_line():
        z.run(f"open {counter}:8", "wait 300", "keys right right")
        before = z.dump()["active"]
        z.run("keys cmd-x")
        cut = z.dump()["active"]
        assert cut["text"].count("\n") == before["text"].count("\n") - 1, cut["text"]
        assert cut["line"] == 8 and cut["line_text"] != before["line_text"], cut
        z.run("keys cmd-v")
        pasted = z.dump()["active"]
        assert pasted["text"] == before["text"], pasted["text"]
        assert pasted["line"] == 9 and pasted["line_text"] == cut["line_text"], pasted
        z.run("keys cmd-z cmd-z")
        assert not z.dump()["active"]["dirty"], "undo should restore the saved state"

    def document_items():
        z.run(f"open {counter}", "keys ctrl-6", "wait 1500", "type reset", "keys enter", "wait 300")
        assert z.dump()["active"]["selection"] == "reset()"

    def find_and_references():
        z.run("keys cmd-shift-f", "wait 200", "type increment", "keys enter", "wait 800")
        assert z.dump()["find_results"] >= 2
        z.run(f"open {counter}:5", "wait 300", keys_right(24), "keys ctrl-shift-cmd-f", "wait 2500")
        assert z.dump()["find_results"] >= 3

    def build_error_and_issue_navigation():
        z.run(f"open {counter}:8", "wait 300", "keys end", "type  + oops", "keys cmd-b")
        s = z.wait_for(lambda s: s["phase"].startswith("Finished"), timeout=180)
        assert "success: false" in s["phase"], s["phase"]
        assert any("oops" in i for i in s["issues"]), s["issues"]
        assert s["navigator"] == "Issues"
        z.run("keys cmd-'", "wait 300")
        assert z.dump()["active"]["path"].endswith("Counter.swift")
        z.run("keys cmd-z", "keys cmd-s")

    def run_on_simulator():
        z.run("keys cmd-r")
        s = z.wait_for(lambda s: any("HelloZ launched" in l for l in s["console_tail"]), timeout=300)
        z.run("keys cmd-.", "wait 1500")
        assert z.dump()["phase"] in ("Stopped",) or z.dump()["phase"].startswith("Finished")

    def debug_on_simulator():
        z.run(f"open {content}:23", "wait 300", "keys cmd-\\", "keys cmd-r")
        s = z.wait_for(lambda s: s["debugger"]["state"].startswith("Stopped"), timeout=300)
        assert "ContentView.swift:23" in s["debugger"]["frames"][0], s["debugger"]
        z.run(f"open {content}:23", "keys cmd-\\", "keys cmd-.", "wait 1500")

    check("sourcekit-lsp ready with compile database (no false errors)", lsp_ready)
    check("Open Quickly (⇧⌘O)", open_quickly)
    check("Jump to Definition across files (⌃⌘J)", definition)
    check("Completion + placeholder Tab", completion)
    check("Comment (⌘/) and undo restores clean state", comment_and_undo)
    check("Cut line (⌘X) and paste above (⌘V) with nothing selected", cut_and_paste_line)
    check("Document Items (⌃6)", document_items)
    check("Find in Workspace (⇧⌘F) and references (⌃⇧⌘F)", find_and_references)
    check("Build error → Issue navigator → next issue (⌘')", build_error_and_issue_navigation)
    if sim:
        check("Run on iOS Simulator (⌘R) with console", run_on_simulator)
        check("Debug on iOS Simulator (breakpoint stop)", debug_on_simulator)
    for path, text in originals.items():
        with open(path, "w") as f:
            f.write(text)


def package_checks(z):
    kit = os.path.join(SAMPLES, "ZKit")
    main = os.path.join(kit, "Sources", "zhello", "main.swift")

    def run_package():
        z.run("keys cmd-r")
        s = z.wait_for(lambda s: s["phase"].startswith("Finished"), timeout=300)
        assert "Hello, zcode!" in s["console_tail"], s["console_tail"]

    def test_package():
        z.run("keys cmd-u")
        s = z.wait_for(lambda s: s["phase"].startswith("Finished"), timeout=300)
        assert s["tests"] and all(t.startswith("Passed") for t in s["tests"]), s["tests"]

    def debug_package():
        z.run(f"open {main}:5", "wait 300", "keys cmd-\\", "keys cmd-r")
        s = z.wait_for(
            lambda s: s["debugger"]["state"].startswith("Stopped") and s["debugger"]["variables"],
            timeout=300,
            interval=0.5,
        )
        assert any(v.startswith("i = ") for v in s["debugger"]["variables"]), s["debugger"]
        z.run("keys f6", "wait 1000")
        assert "main.swift:6" in z.dump()["debugger"]["frames"][0]
        z.run(f"open {main}:5", "keys cmd-\\", "keys ctrl-cmd-y")
        z.wait_for(lambda s: s["phase"].startswith("Finished"), timeout=60)

    check("Swift package: Run on My Mac (⌘R)", run_package)
    check("Swift package: Test (⌘U) → Test navigator", test_package)
    check("Swift package: breakpoint, variables, step over (F6)", debug_package)


def main():
    if not os.path.exists(BINARY):
        sys.exit("build first: cargo build --release")
    sim = "--simulator" in sys.argv
    print("Startup benchmark (warm; the first launch after a rebuild includes macOS's binary scan):")
    for _ in range(2):
        out = subprocess.run([BINARY, "--startup-bench"], capture_output=True, text=True).stdout.strip()
    print("  " + out.splitlines()[-1] if out else "  (no output)")

    print("Xcode project (samples/HelloZ):")
    z = Zcode(os.path.join(SAMPLES, "HelloZ"))
    try:
        xcode_project_checks(z, sim)
    finally:
        z.quit()

    print("Swift package (samples/ZKit):")
    z = Zcode(os.path.join(SAMPLES, "ZKit"))
    try:
        package_checks(z)
    finally:
        z.quit()

    shutil.rmtree(DEBUG_DIR, ignore_errors=True)
    print(f"\n{passed} passed, {failed} failed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

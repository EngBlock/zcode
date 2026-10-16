#!/usr/bin/env python3
"""End-to-end UI checks for zcode, driven through its debug control channel.

    cargo build --release && python3 scripts/e2e.py [--simulator] [--only <text>]

Launches zcode on the sample projects with ZCODE_DEBUG_DIR set, sends
keystrokes/actions, and asserts on the JSON state from `dump`. `--simulator`
also builds, runs and debugs the iOS sample in a simulator (slower).
`--only <text>[,<text>…]` runs just the checks whose names contain one of them.
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
PERF_LOG = os.path.join(WORK, "perf.jsonl")
BINARY = os.path.join(ROOT, "target", "release", "zcode")
DEBUG_DIR = tempfile.mkdtemp(prefix="zcode-e2e-")

passed, failed = 0, 0
ONLY = sys.argv[sys.argv.index("--only") + 1].lower().split(",") if "--only" in sys.argv else None


# Xcode 27 default shortcuts (docs/xcode-inventory.md) → the zcode command
# that must be bound to the same keystroke.
XCODE_SHORTCUTS = [
    ("cmd-o", "zcode::OpenProject", "File ▸ Open…"),
    ("cmd-shift-o", "zcode::OpenQuickly", "File ▸ Open Quickly…"),
    ("cmd-n", "zcode::NewFile", "File ▸ New ▸ File…"),
    ("cmd-alt-a", "zcode::AddFiles", "File ▸ Add Files…"),
    ("cmd-s", "zcode::Save", "File ▸ Save"),
    ("cmd-alt-s", "zcode::SaveAll", "File ▸ Save All"),
    ("cmd-w", "zcode::CloseTab", "File ▸ Close Tab"),
    ("ctrl-alt-cmd-w", "zcode::CloseProject", "File ▸ Close Workspace"),
    ("cmd-d", "zcode::DuplicateLine", "Edit ▸ Duplicate"),
    ("cmd-/", "zcode::ToggleComment", "Editor ▸ Comment Selection"),
    ("cmd-alt-[", "zcode::MoveLineUp", "Editor ▸ Move Line Up"),
    ("cmd-alt-]", "zcode::MoveLineDown", "Editor ▸ Move Line Down"),
    ("ctrl-i", "zcode::ReIndent", "Editor ▸ Structure ▸ Re-Indent"),
    ("ctrl-shift-i", "zcode::FormatFile", "Editor ▸ Structure ▸ Format File"),
    ("ctrl-cmd-e", "zcode::EditAllInScope", "Editor ▸ Edit All in Scope"),
    ("ctrl-alt-cmd-f", "zcode::FixIssue", "Editor ▸ Fix All Issues"),
    ("ctrl-space", "input::ShowCompletions", "Editor ▸ Show Completions"),
    ("cmd-+", "zcode::IncreaseFontSize", "Editor ▸ Font Size ▸ Increase"),
    ("cmd--", "zcode::DecreaseFontSize", "Editor ▸ Font Size ▸ Decrease"),
    ("ctrl-cmd-0", "zcode::ResetFontSize", "Editor ▸ Font Size ▸ Reset"),
    ("ctrl-shift-cmd-l", "zcode::ToggleSoftWrap", "Editor ▸ Wrap Lines"),
    ("cmd-f", "zcode::FindInFile", "Find ▸ Find…"),
    ("cmd-alt-f", "zcode::FindAndReplaceInFile", "Find ▸ Find and Replace…"),
    ("cmd-alt-e", "zcode::SelectNextOccurrence", "Find ▸ Select Next Occurrence"),
    ("cmd-shift-f", "zcode::FindInWorkspace", "Find ▸ Find in Workspace…"),
    ("ctrl-shift-cmd-f", "zcode::FindSelectedSymbol", "Find ▸ Find Selected Symbol in Workspace"),
    ("ctrl-cmd-j", "zcode::JumpToDefinition", "Navigate ▸ Jump to Definition"),
    ("cmd-l", "zcode::JumpToLine", "Navigate ▸ Jump to…"),
    ("ctrl-cmd-left", "zcode::GoBack", "Navigate ▸ Go Back"),
    ("ctrl-cmd-right", "zcode::GoForward", "Navigate ▸ Go Forward"),
    ("ctrl-cmd-up", "zcode::JumpToCounterpart", "Navigate ▸ Jump to Next Counterpart"),
    ("cmd-'", "zcode::NextIssue", "Navigate ▸ Jump to Next Issue"),
    ("cmd-shift-'", "zcode::PreviousIssue", "Navigate ▸ Jump to Previous Issue"),
    ("ctrl-/", "zcode::NextPlaceholder", "Navigate ▸ Jump to Next Placeholder"),
    ("cmd-shift-j", "zcode::RevealInNavigator", "Navigate ▸ Reveal in Project Navigator"),
    ("cmd-shift-[", "zcode::PreviousTab", "Navigate ▸ Show Previous Tab"),
    ("cmd-shift-]", "zcode::NextTab", "Navigate ▸ Show Next Tab"),
    ("cmd-1", "zcode::ShowProjectNavigator", "View ▸ Navigators ▸ Project"),
    ("cmd-2", "zcode::ShowSourceControlNavigator", "View ▸ Navigators ▸ Source Control"),
    ("cmd-4", "zcode::ShowFindNavigator", "View ▸ Navigators ▸ Find"),
    ("cmd-5", "zcode::ShowIssueNavigator", "View ▸ Navigators ▸ Issues"),
    ("cmd-6", "zcode::ShowTestNavigator", "View ▸ Navigators ▸ Tests"),
    ("cmd-7", "zcode::ShowDebugNavigator", "View ▸ Navigators ▸ Debug"),
    ("cmd-8", "zcode::ShowBreakpointNavigator", "View ▸ Navigators ▸ Breakpoints"),
    ("cmd-9", "zcode::ShowReportNavigator", "View ▸ Navigators ▸ Reports"),
    ("cmd-shift-y", "zcode::ToggleDebugArea", "View ▸ Debug Area ▸ Show Debug Area"),
    ("cmd-alt-1", "zcode::ShowFileInspector", "View ▸ Inspectors ▸ File"),
    ("cmd-shift-c", "zcode::ActivateConsole", "View ▸ Debug Area ▸ Activate Console"),
    ("ctrl-6", "zcode::ShowDocumentItems", "View ▸ Editor ▸ Document Items"),
    ("cmd-shift-a", "zcode::QuickActions", "View ▸ Show Quick Actions…"),
    ("cmd-b", "zcode::Build", "Product ▸ Build"),
    ("cmd-r", "zcode::Run", "Product ▸ Run"),
    ("cmd-u", "zcode::Test", "Product ▸ Test"),
    ("ctrl-alt-cmd-u", "zcode::TestSelectedItems", "Product ▸ Perform Action ▸ Test Selected Items"),
    ("ctrl-alt-cmd-g", "zcode::TestAgain", "Product ▸ Perform Action ▸ Test Again"),
    ("ctrl-alt-cmd-k", "zcode::ClearTestResults", "Product ▸ Clear Test Results"),
    ("cmd-.", "zcode::Stop", "Product ▸ Stop"),
    ("cmd-shift-k", "zcode::Clean", "Product ▸ Clean Build Folder"),
    ("ctrl-0", "zcode::ChooseScheme", "Product ▸ Scheme ▸ Choose Scheme"),
    ("ctrl-cmd-]", "zcode::NextScheme", "Product ▸ Scheme ▸ Next Scheme"),
    ("ctrl-cmd-[", "zcode::PreviousScheme", "Product ▸ Scheme ▸ Previous Scheme"),
    ("cmd-<", "zcode::EditScheme", "Product ▸ Scheme ▸ Edit Scheme…"),
    ("cmd-alt-r", "zcode::RunWithOptions", "Product ▸ Run…"),
    ("cmd-alt-u", "zcode::TestWithOptions", "Product ▸ Test…"),
    ("ctrl-shift-0", "zcode::ChooseDestination", "Product ▸ Destination ▸ Choose Destination"),
    ("ctrl-cmd-y", "zcode::DebugContinue", "Debug ▸ Pause / Continue"),
    ("f6", "zcode::StepOver", "Debug ▸ Step Over"),
    ("f7", "zcode::StepInto", "Debug ▸ Step Into"),
    ("f8", "zcode::StepOut", "Debug ▸ Step Out"),
    ("cmd-\\", "zcode::ToggleBreakpoint", "Debug ▸ Breakpoints ▸ Create Breakpoint at Current Line"),
    ("cmd-y", "zcode::ToggleBreakpointsActive", "Debug ▸ Activate Breakpoints"),
    ("cmd-alt-\\", "zcode::CreateSymbolicBreakpoint", "Debug ▸ Breakpoints ▸ Create Symbolic Breakpoint…"),
    ("cmd-k", "zcode::ClearConsole", "Debug ▸ Debug Workflow ▸ Clear Console"),
]


def normalize_keys(keys):
    *mods, key = keys.split("-") if not keys.endswith("--") else keys[:-2].split("-") + ["-"]
    return frozenset(m for m in mods if m), key


class Zcode:
    def __init__(self, project, extra_env=None):
        self.log_offset = 0
        env = dict(os.environ, ZCODE_DEBUG_DIR=DEBUG_DIR, **(extra_env or {}))
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
    if ONLY and not any(part in name.lower() for part in ONLY):
        return
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


def wait_file(path, predicate, timeout=15):
    """Polls a file until predicate(contents) holds; returns the contents."""
    deadline = time.time() + timeout
    while True:
        with open(path) as f:
            text = f.read()
        if predicate(text):
            return text
        if time.time() > deadline:
            raise TimeoutError(f"{os.path.basename(path)} never matched")
        time.sleep(0.1)


def lines_kept(original, edited):
    """Every original line is still there, in order (edits only inserted lines)."""
    rest = iter(original.splitlines())
    want = next(rest, None)
    for line in edited.splitlines():
        if line == want:
            want = next(rest, None)
    return want is None


def group_block(text, name):
    """The first object commented `/* name */` (groups come before targets)."""
    start = text.index(f"/* {name} */ = {{\n")
    return text[start:text.index("\t\t};", start)]


def finished(s):
    return s["phase"].startswith("Finished")


def console_has(z, text, timeout=300):
    """Waits for a console line containing `text`."""
    return z.wait_for(lambda s: any(text in l for l in s["console_tail"]), timeout=timeout)


def console_output(z, command, timeout=60):
    """The console lines after the last `(lldb) <command>`, once there are some."""
    echo = f"(lldb) {command}"

    def lines(s):
        tail = s["console_tail"]
        if echo not in tail:
            return []
        return tail[len(tail) - tail[::-1].index(echo):]

    return lines(z.wait_for(lambda s: lines(s), timeout=timeout, interval=0.3))


def stopped(z, timeout=300, variables=True):
    """Waits for the debugger to stop with frames (and variables)."""
    return z.wait_for(lambda s: s["debugger"]["state"].startswith("Stopped") and s["debugger"]["frames"]
                      and (s["debugger"]["variables"] or not variables), timeout=timeout, interval=0.5)


def debug_run_ends(z, timeout=300):
    """Waits for a debugged program (started after clearing the console) to end."""
    return z.wait_for(lambda s: any(l.startswith("Program ended") for l in s["console_tail"]) and finished(s),
                      timeout=timeout, interval=0.5)


def gutter_row(z, line):
    """Where to click on `line`'s number in the active editor's gutter."""
    g = z.dump()["breakpoints"]["gutter"]
    return g["x"], next(y for l, y in g["rows"] if l == line)


def scheme_editor(z):
    return z.dump()["edit_scheme"]


def run_tests(z, *commands, timeout=300):
    """Starts a test run with `commands` and waits for it to finish."""
    before = z.dump()["reports"]
    z.run(*commands)
    return z.wait_for(lambda s: s["reports"] > before and finished(s), timeout=timeout)


def diamond(z, path, test, timeout=60):
    """The gutter diamond of `test` (a `Class/test` suffix) in `path`."""
    z.run(f"open {path}", "wait 300")
    s = z.wait_for(lambda s: any(d["test"].endswith(test) for d in s["testing"]["gutter"]), timeout=timeout, interval=0.5)
    return next(d for d in s["testing"]["gutter"] if d["test"].endswith(test))


def item_status(s, test):
    """`Passed`, `Failed`, `Skipped` or `NotRun` for a test (`Target/Class/test` suffix)."""
    return next(i.split(" ", 1)[0] for i in s["testing"]["items"] if i.endswith(test))


def empty_trash_item(name):
    """Removes a test file Move to Trash put in ~/.Trash."""
    path = os.path.expanduser(os.path.join("~/.Trash", name))
    if os.path.exists(path):
        os.remove(path)


def xcode_project_checks(z, samples, sim):
    hello = os.path.join(samples, "HelloZ", "HelloZ")
    hello_scheme = os.path.join(samples, "HelloZ", "HelloZ.xcodeproj", "xcshareddata", "xcschemes", "HelloZ.xcscheme")
    hello_scheme_original = open(hello_scheme).read()
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

    def line_clipboard():
        saved = subprocess.run(["pbpaste"], capture_output=True).stdout
        try:
            line = open(counter).read().splitlines()[count_line - 1] + "\n"
            z.run(f"open {counter}:{count_line}", "wait 300", "keys cmd-c", "keys cmd-v", "wait 200")
            s = z.dump()["active"]
            assert line + line in s["text"], "⌘C then ⌘V with no selection should duplicate the line"
            assert s["line"] == count_line + 1, s["line"]
            z.run("keys cmd-z", "keys cmd-x", "wait 200")
            assert z.dump()["active"]["text"].count(line) == 0, "⌘X with no selection should cut the line"
            z.run("keys cmd-z", "wait 200")
            assert not z.dump()["active"]["dirty"], "undo should restore the saved state"
        finally:
            subprocess.run(["pbcopy"], input=saved)

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

    def project_settings():
        pbxproj = os.path.join(samples, "HelloZ", "HelloZ.xcodeproj", "project.pbxproj")
        original = open(pbxproj).read()
        z.run("action zcode::ShowProjectSettings", "wait 300")
        s = z.dump()["project_settings"]
        assert s and s["shown"] and s["target"] == "HelloZ", s
        assert "MARKETING_VERSION = 1.0" in s["fields"], s["fields"]
        z.run("project-setting MARKETING_VERSION 2.5", "wait 200")
        edited = open(pbxproj).read()
        # General fields write every configuration and nothing else.
        assert edited.count("MARKETING_VERSION = 2.5;") == 2, edited
        assert edited.replace("MARKETING_VERSION = 2.5;", "MARKETING_VERSION = 1.0;", 2) == original
        z.run("project-setting MARKETING_VERSION 1.0", "wait 200", "keys cmd-w")
        assert open(pbxproj).read() == original, "restoring the value should restore the file byte for byte"
        assert not z.dump()["project_settings"]

    def synchronized_folders_untouched():
        pbxproj = os.path.join(samples, "HelloZ", "HelloZ.xcodeproj", "project.pbxproj")
        original = open(pbxproj).read()
        z.run(f"inspect {counter}")
        s = z.wait_for(lambda s: s["inspector"]["tracking"], timeout=15, interval=0.3)
        assert s["inspector"]["tracking"] == {"synchronized": "HelloZ", "targets": ["HelloZ"]}, s["inspector"]
        name = f"ZcodeE2EScratch{os.getpid()}.swift"
        scratch = os.path.join(hello, name)
        z.run(f"new-file {scratch}", "wait 1000")
        z.run(f"rename {scratch} -> Renamed{name}", "wait 1000")
        z.run(f"trash {os.path.join(hello, 'Renamed' + name)}", "wait 1000")
        empty_trash_item("Renamed" + name)
        assert open(pbxproj).read() == original, "synchronized folders need no project edits"

    def unit_tests_on_simulator():
        z.run("keys cmd-u")
        s = z.wait_for(lambda s: s["phase"].startswith("Finished"), timeout=300)
        assert "success: true" in s["phase"], s["phase"]
        assert sorted(s["tests"]) == [
            "Passed CounterTests/incrementAddsOne()",
            "Passed CounterTests/resetClearsCount()",
            "Passed LegacyTests/testIncrementTwice",
            "Passed LegacyTests/testStartsAtZero",
        ], s["tests"]

    def single_tests_on_simulator():
        legacy = os.path.join(samples, "HelloZ", "HelloZTests", "LegacyTests.swift")
        counter_tests = os.path.join(samples, "HelloZ", "HelloZTests", "CounterTests.swift")
        z.run("action zcode::ClearTestResults")
        d = diamond(z, legacy, "LegacyTests/testStartsAtZero")
        s = run_tests(z, f"click {d['x']} {d['y']}")
        assert "success: true" in s["phase"] and s["tests"] == ["Passed LegacyTests/testStartsAtZero"], s["tests"]
        # A Swift Testing suite through the injected bundle's scope.
        d = diamond(z, counter_tests, "HelloZTests/CounterTests")
        s = run_tests(z, f"click {d['x']} {d['y']}")
        assert sorted(s["tests"])[:2] == ["Passed CounterTests/incrementAddsOne()", "Passed CounterTests/resetClearsCount()"], s["tests"]
        assert len(s["tests"]) == 3, s["tests"]

    def test_plan_on_simulator():
        s = z.wait_for(lambda s: s["testing"]["plans"] == ["HelloZ", "Smoke"], timeout=30)
        assert s["testing"]["plan"] == "HelloZ", s["testing"]
        z.run("action zcode::ChooseTestPlan", "wait 300", "type Smoke", "keys enter", "wait 300")
        assert z.dump()["testing"]["plan"] == "Smoke"
        s = run_tests(z, "keys cmd-u")
        # Smoke skips LegacyTests/testIncrementTwice().
        assert "success: true" in s["phase"], s["phase"]
        assert not any("testIncrementTwice" in t for t in s["tests"]), s["tests"]
        assert "Passed LegacyTests/testStartsAtZero" in s["tests"], s["tests"]
        z.run("action zcode::ChooseTestPlan", "wait 300", "type HelloZ", "keys enter", "wait 300")

    def run_on_simulator():
        # Edit Scheme's arguments and environment reach simctl launch
        # (SIMCTL_CHILD_ variables).
        z.run("keys cmd-<", "wait 300", "edit-scheme argument -sim \"two words\"",
              "edit-scheme variable HELLOZ_MODE=scheme", "edit-scheme close")
        z.run("keys cmd-r")
        console_has(z, "HelloZ launched")
        console_has(z, "arguments: -sim two words HELLOZ_MODE=scheme", timeout=20)
        z.run("keys cmd-.", "wait 1500")
        phase = z.dump()["phase"]
        assert phase == "Stopped" or phase.startswith("Finished"), phase

    def debug_on_simulator():
        # Under the debugger, with Main Thread Checker inserted (the default).
        z.run(f"open {content}:{appear_line}", "wait 300", "keys cmd-\\", "keys cmd-r")
        s = z.wait_for(lambda s: s["debugger"]["state"].startswith("Stopped") and s["debugger"]["frames"], timeout=300)
        assert f"ContentView.swift:{appear_line}" in s["debugger"]["frames"][0], s["debugger"]
        assert not any("Main Thread Checker not found" in l for l in s["console_tail"]), s["console_tail"]
        # The LLDB console works on the attached simulator app.
        z.run("console p 6 * 7")
        assert any("42" in l for l in console_output(z, "p 6 * 7", timeout=120)), z.dump()["console_tail"]
        z.run("console bt 1")
        assert any("ContentView.swift" in l for l in console_output(z, "bt 1")), z.dump()["console_tail"]
        z.run(f"open {content}:{appear_line}", "keys cmd-\\", "keys cmd-.", "wait 1500")
        # Restore the scheme the run check edited, byte for byte.
        z.run("keys cmd-<", "wait 300", "edit-scheme remove-argument -sim \"two words\"",
              "edit-scheme remove-variable HELLOZ_MODE", "edit-scheme close")
        assert open(hello_scheme).read() == hello_scheme_original, "the scheme is as it was"

    def shortcut_parity():
        keymap = json.loads(z.run("keymap")[-1])
        bound = {action: {normalize_keys(k) for k in keys} for action, keys in keymap.items()}
        missing = [
            f"{label} ({keys})"
            for keys, action, label in XCODE_SHORTCUTS
            if normalize_keys(keys) not in bound.get(action, set())
        ]
        assert not missing, missing
        print(f"    {len(XCODE_SHORTCUTS)} Xcode default shortcuts bound to the matching zcode command")

    def memory_monitoring():
        s = z.wait_for(lambda s: s["memory"]["samples"] >= 3 and s["memory"]["language_server_mb"], timeout=60)
        memory = s["memory"]
        assert memory["footprint_mb"] > 0 and "LSP" in memory["status_label"], memory
        deadline = time.time() + 30
        samples = []
        while time.time() < deadline:
            with open(PERF_LOG) as f:
                samples = [json.loads(line) for line in f]
            if len(samples) >= 3 and any(x["language_server_footprint_mb"] for x in samples):
                break
            time.sleep(1)
        assert len(samples) >= 3 and samples[-1]["footprint_mb"] > 0, samples[-3:]
        assert any(x["language_server_footprint_mb"] for x in samples), samples[-3:]
        print(f"    {memory['status_label']} · {memory['samples']} samples · baseline {memory['baseline_footprint_mb']} MB · {len(samples)} lines in --perf-log")

    check("Xcode default shortcuts (from the Xcode 27 inventory)", shortcut_parity)
    check("sourcekit-lsp ready with compile database (no false errors)", lsp_ready)
    check("Memory monitoring: sampler, LSP + SourceKitService, perf log", memory_monitoring)
    check("Open Quickly (⇧⌘O)", open_quickly)
    check("Jump to Definition across files (⌃⌘J)", definition)
    check("Completion + placeholder Tab", completion)
    check("Comment (⌘/) and undo restores clean state", comment_and_undo)
    check("Cut / copy / paste whole line with no selection (⌘X ⌘C ⌘V)", line_clipboard)
    check("Document Items (⌃6)", document_items)
    check("Find in Workspace (⇧⌘F) and references (⌃⇧⌘F)", find_and_references)
    check("Project settings: edit a General field, written to project.pbxproj in place", project_settings)
    check("Build error → Issue navigator → next issue (⌘')", build_error_and_issue_navigation)
    check("Synchronized folders: New File / Rename / Trash leave project.pbxproj alone", synchronized_folders_untouched)
    if sim:
        check("Unit tests in iOS Simulator (⌘U)", unit_tests_on_simulator)
        check("Simulator: one test and one suite from gutter diamonds (XCTest scope)", single_tests_on_simulator)
        check("Simulator: test plan (Smoke) honored by the in-simulator runner", test_plan_on_simulator)
        check("Run on iOS Simulator (⌘R) with console", run_on_simulator)
        check("Debug on iOS Simulator (breakpoint stop, LLDB console)", debug_on_simulator)


def available_simulator_udid():
    """An available iOS simulator, which the fake devices pretend to be so
    xcodebuild can really build for them."""
    out = subprocess.run(["xcrun", "simctl", "list", "devices", "available", "--json"], capture_output=True, text=True).stdout
    devices = json.loads(out)["devices"]
    return next(d["udid"] for runtime, ds in sorted(devices.items(), reverse=True) if ".iOS-" in runtime for d in ds)


def device_checks(samples):
    """Physical devices through a fake devicectl (scripts/fake_devicectl.py)
    serving tests/fixtures/devicectl/list-devices.json. "Test iPhone" has an
    iOS simulator's UDID, so xcodebuild builds for it and zcode's device path
    (install, lock wait, launch, console, Stop, debugger attach) runs in full;
    real hardware isn't needed."""
    state = os.path.join(WORK, "fake-devicectl")
    os.makedirs(state, exist_ok=True)
    fake_udid = "00008150-0000000000000001"
    udid = available_simulator_udid()
    with open(os.path.join(ROOT, "tests", "fixtures", "devicectl", "list-devices.json")) as f:
        devices_json = f.read().replace(fake_udid, udid)
    devices_path = os.path.join(state, "devices.json")

    def set_devices(drop=()):
        document = json.loads(devices_json)
        document["result"]["devices"] = [d for d in document["result"]["devices"]
                                         if d["properties"]["state"]["name"] not in drop]
        with open(devices_path + ".tmp", "w") as f:
            json.dump(document, f)
        os.rename(devices_path + ".tmp", devices_path)

    def calls():
        with open(os.path.join(state, "calls.jsonl")) as f:
            return [json.loads(l) for l in f]

    def processes():
        try:
            with open(os.path.join(state, "processes.json")) as f:
                return json.load(f)
        except FileNotFoundError:
            return []

    set_devices()
    project = os.path.join(WORK, "Devices", "HelloZ")
    shutil.copytree(os.path.join(samples, "HelloZ"), project)
    z = Zcode(project, {"ZCODE_DEVICECTL": os.path.join(ROOT, "scripts", "fake_devicectl.py"), "FAKE_DEVICECTL_DIR": state})
    try:
        def menu():
            s = z.wait_for(lambda s: any(l.startswith("Build |") for l in s["destination_menu"])
                           and not any("watchOS" in l for l in s["destination_menu"]), timeout=90)
            m = s["destination_menu"]
            assert m[:5] == [
                "iOS Devices | Test iPhone | iOS 27.2 · Connected via network",
                "iOS Devices | Test iPhone (USB) | iOS 27.2 · Connected via USB",
                "iOS Devices | Old iPhone (Unavailable) | iOS 18.6 · Unavailable",
                "iOS Devices | Test iPad (Developer Mode disabled) | iOS 26.4 · Developer Mode disabled",
                "Build | Any iOS Device (arm64) | Build only",
            ], m
            # The scheme builds for iOS only: no watch, TV or Vision Pro; simulators follow.
            assert all(l.split(" | ")[0].endswith("Simulators") for l in m[5:]) and len(m) > 5, m

        def hot_plug():
            set_devices(drop=("Test iPhone (USB)",))
            # Polled every few seconds while the window is active.
            z.wait_for(lambda s: not any("(USB)" in l for l in s["destination_menu"]), timeout=20, interval=0.5)
            set_devices()
            z.wait_for(lambda s: any("(USB)" in l for l in s["destination_menu"]), timeout=20, interval=0.5)

        def unavailable_device():
            z.run("destination Test iPad", "keys cmd-r")
            s = z.wait_for(finished, timeout=30)
            assert "success: false" in s["phase"], s["phase"]
            assert any("Developer Mode is disabled on Test iPad" in i for i in s["issues"]), s["issues"]
            assert not any("install" in c["args"] for c in calls()), "nothing is built or installed"

        def run_and_stop():
            z.wait_for(lambda s: s["scheme_options"], timeout=60)
            z.run("keys cmd-<", "wait 300", "edit-scheme argument -onDevice", "edit-scheme variable HELLOZ_MODE=device",
                  "edit-scheme close")
            open(os.path.join(state, "locked"), "w").close()
            z.run(f"destination {udid}", "keys cmd-r")
            assert z.dump()["destination"] == "Test iPhone (iOS 27.2)"
            console_has(z, "Test iPhone is locked. Unlock it to continue…", timeout=300)
            os.remove(os.path.join(state, "locked"))
            s = console_has(z, "arguments: -onDevice HELLOZ_MODE=device", timeout=60)
            assert s["phase"].startswith("Running"), s["phase"]
            tail = s["console_tail"]
            assert "HelloZ launched" in tail and not any("Launched application with" in l for l in tail), tail
            install = next(c for c in calls() if c["args"][:3] == ["device", "install", "app"])
            assert install["args"][3:5] == ["--device", udid] and install["args"][5].endswith("HelloZ.app"), install
            launch = [c for c in calls() if c["args"][:3] == ["device", "process", "launch"]][-1]
            assert "--console" in launch["args"] and "--start-stopped" not in launch["args"], launch
            assert launch["args"][-2:] == ["dev.zcode.HelloZ", "-onDevice"], launch
            assert launch["env"] == {"DEVICECTL_CHILD_HELLOZ_MODE": "device"}, launch
            assert processes(), "the app runs"
            z.run("keys cmd-.")
            z.wait_for(lambda s: s["phase"] == "Stopped" and not processes(), timeout=15, interval=0.3)
            build_log = open(z.dump()["last_log"]).read()
            assert f"-destination id={udid} -skipMacroValidation -skipPackagePluginValidation -allowProvisioningUpdates -allowProvisioningDeviceRegistration build" in build_log

        def debug_attach():
            content = os.path.join(project, "HelloZ", "ContentView.swift")
            line = line_of(content, 'print("HelloZ launched")')
            z.run(f"open {content}:{line}", "wait 300", "keys cmd-\\", "keys cmd-r")
            # The fake device isn't real, so lldb's attach fails; the commands
            # are what attaches on hardware (verified by hand on an iPhone).
            s = console_has(z, "(lldb) device process attach --pid 4242", timeout=300)
            launch = [c for c in calls() if c["args"][:3] == ["device", "process", "launch"]][-1]
            assert "--start-stopped" in launch["args"], launch
            tail = s["console_tail"]
            assert "Attaching to process 4242…" in tail, tail
            assert "(lldb) device select FAD3EBA8-0000-0000-0000-000000000001" in tail, tail
            assert "(lldb) settings set target.memory-module-load-level partial" in tail, tail
            assert any(l.startswith('(lldb) settings set target.exec-search-paths "') and l.endswith('HelloZ.app"') for l in tail), tail
            z.wait_for(lambda s: s["debugger"]["state"] == "Exited", timeout=120)
            z.run("keys cmd-\\", "keys cmd-.", "wait 500")
            z.wait_for(lambda s: not processes(), timeout=15, interval=0.3)

        def signing_error():
            z.run("destination generic:iOS", "keys cmd-b")
            s = z.wait_for(finished, timeout=300)
            assert "success: false" in s["phase"], s["phase"]
            issues = s["issues"]
            assert ('Error HelloZ.xcodeproj:0 Signing for "HelloZ" requires a development team. '
                    "Set the target's Development Team (your team ID) in the project settings.") in issues, issues
            assert s["navigator"] == "Issues"
            assert "generic/platform=iOS" in open(s["last_log"]).read()

        check("Devices: the destination menu lists devices by OS with their state, Any iOS Device, then simulators", menu)
        check("Devices: plugging and unplugging a device updates the menu", hot_plug)
        check("Devices: Run on an unavailable device says why without building", unavailable_device)
        check("Devices: Run signs, installs, waits for unlock, launches with the scheme's arguments and environment; Stop terminates", run_and_stop)
        check("Devices: Run with breakpoints launches suspended and attaches lldb through CoreDevice", debug_attach)
        check("Devices: a signing error for Any iOS Device shows in the Issue navigator with where to fix it", signing_error)
    finally:
        z.quit()


def classic_project_checks(z, samples):
    root = os.path.join(samples, "HelloClassic")
    app = os.path.join(root, "HelloClassic")
    pbxproj = os.path.join(root, "HelloClassic.xcodeproj", "project.pbxproj")
    original = open(pbxproj).read()
    badge = os.path.join(app, "Views", "Badge.swift")

    def new_file():
        z.wait_for(lambda s: s["xcode_projects"], timeout=30, interval=0.3)
        os.makedirs(os.path.dirname(badge))  # the New File panel can create folders
        z.run(f"new-file {badge}")
        text = wait_file(pbxproj, lambda t: "Badge.swift in Sources" in t)
        assert "/* Views */ = {" in text and "path = Views;" in text, "a group for the new folder"
        assert lines_kept(original, text), "only inserted lines"
        assert z.dump()["active"]["path"].endswith("Badge.swift")

    def target_membership():
        z.run(f"inspect {badge}")
        on, both = ["HelloClassic: on", "HelloClassicTests: off"], ["HelloClassic: on", "HelloClassicTests: on"]
        targets = lambda s: (s["inspector"]["tracking"] or {}).get("targets")
        z.wait_for(lambda s: targets(s) == on, timeout=15, interval=0.3)
        before = open(pbxproj).read()
        z.run("membership HelloClassicTests on")
        wait_file(pbxproj, lambda t: t.count("/* Badge.swift in Sources */ = {isa = PBXBuildFile") == 2)
        z.wait_for(lambda s: targets(s) == both, timeout=15, interval=0.3)
        z.run("membership HelloClassicTests off")
        assert wait_file(pbxproj, lambda t: t == before), "unchecking restores the file byte for byte"

    def rename_and_move():
        z.run(f"rename {os.path.join(app, 'Model', 'Greeter.swift')} -> Greeting.swift")
        wait_file(pbxproj, lambda t: "/* Greeting.swift in Sources */" in t and "Greeter.swift" not in t)
        z.run(f"rename {os.path.join(app, 'ContentView.swift')} -> Views/ContentView.swift")
        text = wait_file(pbxproj, lambda t: "ContentView.swift" in group_block(t, "Views"))
        assert os.path.exists(os.path.join(app, "Views", "ContentView.swift"))
        assert "ContentView.swift" not in group_block(text, "HelloClassic"), "moved out of its old group"

    def trash():
        name = f"ZcodeE2EJunk{os.getpid()}.swift"
        junk = os.path.join(app, name)
        z.run(f"new-file {junk}")
        wait_file(pbxproj, lambda t: f"{name} in Sources" in t)
        z.run(f"trash {junk}")
        wait_file(pbxproj, lambda t: name not in t)
        empty_trash_item(name)

    def add_files():
        extras = os.path.join(WORK, "external", "Extras")
        os.makedirs(extras)
        with open(os.path.join(extras, "Extra.swift"), "w") as f:
            f.write("struct Extra { var name = \"extra\" }\n")
        with open(os.path.join(extras, "extra.json"), "w") as f:
            f.write("{}\n")
        z.run(f"add-files {extras} -> {app}", "wait 300")
        s = z.dump()["add_files"]
        assert s and s["copy"] and s["targets"] == ["HelloClassic: on", "HelloClassicTests: off"], s
        z.run("keys enter")
        text = wait_file(pbxproj, lambda t: "Extra.swift in Sources" in t and "extra.json in Resources" in t)
        assert "/* Extras */ = {" in text
        assert os.path.exists(os.path.join(app, "Extras", "Extra.swift")), "copied into the project"
        assert not z.dump()["add_files"]

    def builds():
        z.run("keys cmd-b")
        s = z.wait_for(lambda s: s["phase"].startswith("Finished"), timeout=300)
        assert "success: true" in s["phase"], (s["phase"], s["issues"][:5])

    scheme = os.path.join(root, "HelloClassic.xcodeproj", "xcshareddata", "xcschemes", "HelloClassic.xcscheme")
    scheme_original = open(scheme).read()

    def edit_scheme_run_options():
        z.wait_for(lambda s: s["scheme_options"], timeout=60, interval=0.5)
        z.run("keys cmd-<", "wait 300")
        s = scheme_editor(z)
        assert s and s["action"] == "Run" and s["storage"] == "Shared scheme · HelloClassic.xcscheme", s
        z.run("keys escape", "wait 200")
        assert not z.dump()["modal_open"], "Escape closes the sheet"
        z.run("keys cmd-<", "wait 300", "edit-scheme tab arguments", "edit-scheme argument -verbose \"two words\"",
              "edit-scheme argument -skipped", "edit-scheme toggle-argument -skipped",
              "edit-scheme variable GREETING=hi there", "edit-scheme variable OFF=1", "edit-scheme toggle-variable OFF",
              "edit-scheme diagnostic zombie_objects on", "edit-scheme close")
        text = wait_file(scheme, lambda t: "GREETING" in t)
        assert lines_kept(scheme_original, text), "only inserted lines"
        for expected in ['argument = "-verbose &quot;two words&quot;"', 'argument = "-skipped"\n            isEnabled = "NO"',
                         'key = "GREETING"\n            value = "hi there"\n            isEnabled = "YES"',
                         'key = "OFF"\n            value = "1"\n            isEnabled = "NO"', 'key = "NSZombieEnabled"']:
            assert expected in text, expected
        launch = text[text.index("<LaunchAction"):text.index("</LaunchAction>")]
        assert "GREETING" in launch and "<EnvironmentVariables>" not in text[text.index("<TestAction"):text.index("</TestAction>")]
        o = z.dump()["scheme_options"]["run"]
        assert o["lists"]["environment"] == ["GREETING=hi there", "(off) OFF=1"] and "zombie_objects" in o["diagnostics"], o

    def run_mac_app_with_options():
        z.run("keys cmd-r")
        # Split like a shell, the disabled ones left out; streamed live (a pseudo-terminal).
        console_has(z, "arguments: -verbose two words GREETING=hi there")
        z.run("keys cmd-.", "wait 1500")

    def edit_scheme_restores_file():
        z.run("keys cmd-<", "wait 300", "edit-scheme action test", "edit-scheme configuration Release", "edit-scheme close")
        text = wait_file(scheme, lambda t: 'buildConfiguration = "Release"' in t[t.index("<TestAction"):])
        assert 'buildConfiguration = "Debug"' in text[text.index("<LaunchAction"):], "the Run action keeps Debug"
        assert z.dump()["configuration"] == "Debug"
        z.run("keys cmd-<", "wait 300", "edit-scheme action test", "edit-scheme configuration Debug", "edit-scheme action run",
              "edit-scheme remove-argument -verbose \"two words\"", "edit-scheme remove-argument -skipped",
              "edit-scheme remove-variable GREETING", "edit-scheme remove-variable OFF",
              "edit-scheme diagnostic zombie_objects off", "edit-scheme close")
        assert wait_file(scheme, lambda t: t == scheme_original), "back byte for byte"

    check("New File in a new folder → file reference, group and Sources build file", new_file)
    check("File inspector (⌥⌘1): Target Membership on and off", target_membership)
    check("Rename and move (Rename to Views/…) update references and groups", rename_and_move)
    check("Move to Trash removes the reference and build file", trash)
    check("Add Files (⌥⌘A): copy a folder in as a group, Sources + Resources", add_files)
    check("xcodebuild builds the edited project (⌘B)", builds)
    check("Edit Scheme (⌘<): arguments, environment, diagnostics written into the shared .xcscheme", edit_scheme_run_options)
    check("Run on My Mac gets the scheme's arguments and environment", run_mac_app_with_options)
    check("Edit Scheme: Test configuration, then everything undone restores the file byte for byte", edit_scheme_restores_file)

    def debug_mac_app():
        # Moved into Views/ by the rename check (when it ran).
        content = next(p for p in (os.path.join(app, "Views", "ContentView.swift"), os.path.join(app, "ContentView.swift"))
                       if os.path.exists(p))
        launched = line_of(content, 'print("HelloClassic launched")')
        arguments = line_of(content, 'print("arguments:')
        z.run(f"open {content}:{launched}", "wait 300", "keys cmd-\\")
        z.run(f"open {content}:{arguments}", "wait 300", "keys cmd-\\", "action zcode::EditBreakpoint", "wait 300",
              "breakpoint add-action log launched with @CommandLine.arguments.count@ argument(s), hit %H",
              "breakpoint auto-continue on", "breakpoint close")
        z.run("action zcode::ClearConsole", "keys cmd-r")
        s = stopped(z, variables=False)
        assert f"ContentView.swift:{launched}" in s["debugger"]["frames"][0], s["debugger"]
        z.run("console p ProcessInfo.processInfo.processName")
        out = console_output(z, "p ProcessInfo.processInfo.processName", timeout=120)
        assert any('"HelloClassic"' in l for l in out), out
        # The log message prints and the app runs on without stopping.
        z.run("keys ctrl-cmd-y")
        s = console_has(z, "launched with 1 argument(s), hit 1", timeout=60)
        console_has(z, "arguments: ", timeout=30)
        assert z.dump()["debugger"]["state"] == "Running", z.dump()["debugger"]
        z.run("keys cmd-.", "wait 1500", "action zcode::DeleteAllBreakpoints")

    check("Mac app under lldb-dap: breakpoint stop, LLDB console, auto-continuing log message", debug_mac_app)

    suite = os.path.join(root, "HelloClassicTests", "GreeterSuite.swift")
    xctests = os.path.join(root, "HelloClassicTests", "GreeterTests.swift")

    def gutter_diamonds():
        z.run(f"open {suite}", "wait 300")
        s = z.wait_for(lambda s: len(s["testing"]["gutter"]) == 3, timeout=60, interval=0.5)
        lines = [(d["line"], d["test"]) for d in s["testing"]["gutter"]]
        assert lines == [
            (line_of(suite, "struct GreeterSuite"), "HelloClassicTests/GreeterSuite"),
            (line_of(suite, "func greets"), "HelloClassicTests/GreeterSuite/greets()"),
            (line_of(suite, "func world"), "HelloClassicTests/GreeterSuite/world()"),
        ], lines
        assert all(d["status"] == "NotRun" for d in s["testing"]["gutter"])
        z.run("screenshot " + os.path.join(WORK, "diamonds.png"))

    def run_one_from_gutter():
        d = diamond(z, suite, "GreeterSuite/world()")
        s = run_tests(z, f"click {d['x']} {d['y']}")
        assert "success: true" in s["phase"], s["phase"]
        # xcodebuild ran just that test (-only-testing); its display name maps back.
        assert len(s["tests"]) == 1 and s["tests"][0].startswith("Passed GreeterSuite/") and "world" in s["tests"][0], s["tests"]
        assert s["testing"]["last"] == "GreeterSuite/world()", s["testing"]
        assert item_status(s, "GreeterSuite/world()") == "Passed" and item_status(s, "GreeterSuite/greets()") == "NotRun"
        s = z.wait_for(lambda s: any(d["status"] == "Passed" for d in s["testing"]["gutter"]), timeout=10, interval=0.3)
        assert [d["status"] for d in s["testing"]["gutter"]] == ["NotRun", "NotRun", "Passed"], s["testing"]["gutter"]

    def test_at_cursor_and_again():
        s = run_tests(z, f"open {xctests}:{line_of(xctests, 'XCTAssertEqual')}", "wait 300", "keys ctrl-alt-cmd-u")
        assert s["testing"]["last"] == "GreeterTests/testGreeting", s["testing"]
        assert item_status(s, "GreeterTests/testGreeting") == "Passed", s["testing"]["items"]
        assert item_status(s, "HelloClassicTests/GreeterTests") == "Passed", "a class rolls up its tests"
        s = run_tests(z, "keys ctrl-alt-cmd-g")
        assert s["testing"]["last"] == "GreeterTests/testGreeting", s["testing"]
        # The whole class, as the Test navigator's play button runs it.
        s = run_tests(z, "run-test HelloClassicTests/GreeterSuite")
        assert s["testing"]["last"] == "GreeterSuite", s["testing"]
        assert item_status(s, "HelloClassicTests/GreeterSuite") == "Passed", s["testing"]["items"]

    def test_plan():
        s = z.wait_for(lambda s: s["testing"]["plans"] == ["HelloClassic", "Greeting"], timeout=30)
        assert s["testing"]["plan"] == "HelloClassic", s["testing"]
        z.run("action zcode::ChooseTestPlan", "wait 300", "type Greeting", "keys enter", "wait 300")
        assert z.dump()["testing"]["plan"] == "Greeting"
        s = run_tests(z, "keys cmd-u")
        # The Greeting plan selects only GreeterTests (xcodebuild -testPlan).
        assert "success: true" in s["phase"], s["phase"]
        assert s["tests"] == ["Passed GreeterTests/testGreeting"] or s["tests"] == ["Passed GreeterTests/testGreeting()"], s["tests"]
        z.run("action zcode::ChooseTestPlan", "wait 300", "type HelloClassic", "keys enter", "wait 300")
        z.run("action zcode::ClearTestResults")
        assert all(i.startswith("NotRun") for i in z.dump()["testing"]["items"])

    check("Test diamonds in the gutter for Swift Testing suites and tests", gutter_diamonds)
    check("Click a diamond: xcodebuild -only-testing runs one Swift Testing test", run_one_from_gutter)
    check("Test Selected Items at the cursor (⌃⌥⌘U), Test Again (⌃⌥⌘G), run a suite", test_at_cursor_and_again)
    check("Test plans: choose Greeting, ⌘U runs only its tests (xcodebuild -testPlan)", test_plan)


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

    tests_dir = os.path.join(kit, "Tests", "ZKitTests")
    swift_tests = os.path.join(tests_dir, "GreeterTests.swift")
    xctests = os.path.join(tests_dir, "GreeterXCTests.swift")

    def package_single_tests():
        d = diamond(z, swift_tests, "ZKitTests/excitedGreeting()")
        s = run_tests(z, f"click {d['x']} {d['y']}")
        assert s["tests"] == ["Passed excitedGreeting()"], s["tests"]
        s = run_tests(z, "run-test ZKitTests/GreeterXCTests")
        assert s["tests"][-1] == "Passed GreeterXCTests/testPlainGreeting", s["tests"]
        assert len(s["tests"]) == 2, s["tests"]

    def rerun_failed():
        original = open(xctests).read()
        try:
            with open(xctests, "w") as f:
                f.write(original.replace('"Hello, C."', '"Hello, D."'))
            z.run(f"open {xctests}", "wait 3500")  # reloads from disk
            s = run_tests(z, "keys cmd-u")
            assert "success: false" in s["phase"], s["phase"]
            assert item_status(s, "GreeterXCTests/testPlainGreeting") == "Failed", s["testing"]["items"]
            assert item_status(s, "ZKitTests/plainGreeting()") == "Passed", s["testing"]["items"]
            with open(xctests, "w") as f:
                f.write(original)
            z.wait_for(lambda s: s["active"]["text"] == original, timeout=15, interval=0.5)
            s = run_tests(z, "action zcode::RerunFailedTests")
            assert "success: true" in s["phase"], s["phase"]
            assert s["testing"]["last"] == "GreeterXCTests/testPlainGreeting", s["testing"]
            assert item_status(s, "GreeterXCTests/testPlainGreeting") == "Passed", s["testing"]["items"]
        finally:
            with open(xctests, "w") as f:
                f.write(original)

    check("Swift package: a test from the gutter and an XCTest class (swift test --filter)", package_single_tests)
    check("Swift package: re-run failed tests", rerun_failed)
    check("Swift package: Run on My Mac (⌘R)", run_package)
    check("Swift package: Test (⌘U) → Test navigator", test_package)
    check("Swift package: breakpoint, variables, step over (F6)", debug_package)

    tick_line = line_of(main, 'print("tick')
    throw_line = line_of(main, "throw Unverified")
    greeting_line = line_of(main, "print(greeter.greeting")

    def breakpoint_gutter():
        z.run(f"open {main}:{tick_line}", "wait 400")
        x, y = gutter_row(z, tick_line)
        z.run(f"click {x} {y}", "wait 200")
        s = z.dump()
        assert [(b["line"], b["enabled"]) for b in s["breakpoints"]["list"]] == [(tick_line, True)], s["breakpoints"]
        assert [m["line"] for m in s["breakpoints"]["markers"]] == [tick_line], s["breakpoints"]["markers"]
        z.run(f"click {x} {y}", "wait 200")
        assert not z.dump()["breakpoints"]["list"][0]["enabled"], "a click disables it"
        z.run(f"click {x} {y}", "wait 200")
        assert z.dump()["breakpoints"]["list"][0]["enabled"], "and enables it"
        z.run("screenshot " + os.path.join(WORK, "breakpoint-gutter.png"))
        # Lines inserted above move it down; undoing them moves it back.
        z.run(f"open {main}:1", "keys enter enter", "wait 200")
        assert z.dump()["breakpoints"]["list"][0]["line"] == tick_line + 2, z.dump()["breakpoints"]["list"]
        for _ in range(3):
            if not z.dump()["active"]["dirty"]:
                break
            z.run("keys cmd-z", "wait 200")
        assert z.dump()["breakpoints"]["list"][0]["line"] == tick_line, z.dump()["breakpoints"]["list"]
        # A double-click edits it (and leaves it enabled); a right-click shows its menu.
        z.run(f"open {main}:{tick_line}", "wait 300")
        x, y = gutter_row(z, tick_line)
        z.run(f"double-click {x} {y}", "wait 300")
        s = z.dump()
        assert s["breakpoints"]["editor"]["title"] == f"main.swift:{tick_line}" and s["breakpoints"]["list"][0]["enabled"], s["breakpoints"]
        z.run("screenshot " + os.path.join(WORK, "breakpoint-editor.png"), "breakpoint close")
        z.run(f"right-click {x} {y}", "wait 300")
        assert z.dump()["breakpoints"]["menu_open"], "the breakpoint's menu"
        z.run("keys escape", "wait 200")

    def conditional_breakpoint_and_console():
        z.run(f"open {main}:{tick_line}", "wait 300", "action zcode::EditBreakpoint", "wait 300",
              "breakpoint condition i == 2", "breakpoint close")
        assert z.dump()["breakpoints"]["list"][0]["condition"] == "i == 2"
        z.run("action zcode::ClearConsole", "keys cmd-r")
        s = stopped(z)
        assert "i = 2" in s["debugger"]["variables"] and "tick 1" in s["console_tail"] and "tick 2" not in s["console_tail"], s
        assert s["console"]["prompt"], s["console"]
        # The LLDB console: po, p, expr, v and any lldb command.
        z.run("console po i")
        assert console_output(z, "po i") == ["2"], z.dump()["console_tail"]
        z.run("console p i * 10")
        assert any("20" in l for l in console_output(z, "p i * 10")), z.dump()["console_tail"]
        z.run("console expr greeter.name")
        assert any('"zcode"' in l for l in console_output(z, "expr greeter.name")), z.dump()["console_tail"]
        z.run("console v i")
        assert console_output(z, "v i") == ["(Int) i = 2"], z.dump()["console_tail"]
        z.run("console bt 1")
        assert any("main.swift:" in l for l in console_output(z, "bt 1")), z.dump()["console_tail"]
        # ↑/↓ walk the history at the (lldb) prompt; Return runs the line again.
        z.run("action zcode::ActivateConsole", "wait 200", "keys up", "wait 100")
        assert z.dump()["console"]["input"] == "bt 1", z.dump()["console"]
        z.run("keys up", "keys up", "wait 100")
        assert z.dump()["console"]["input"] == "expr greeter.name", z.dump()["console"]
        z.run("keys down", "wait 100", "keys enter", "wait 300")
        assert z.dump()["console_tail"].count("(lldb) v i") == 2, z.dump()["console_tail"]
        # The Variables view's Print Description, and data tips on hover.
        z.run("variable i describe")
        s = z.wait_for(lambda s: "Printing description of i:" in s["console_tail"], timeout=30, interval=0.3)
        tail = s["console_tail"]
        assert tail[tail.index("Printing description of i:") + 1:][:1] == ["2"], tail
        z.run(f"open {main}:{tick_line}", "wait 200", keys_right(column_of(main, "\\(i)", 2)))
        tip = json.loads(z.run("hover")[-1])["hover"]
        assert "i: Int = 2" in tip, tip
        z.run(f"open {main}:{greeting_line}", "wait 200", keys_right(column_of(main, "greeter.greeting")))
        tip = json.loads(z.run("hover")[-1])["hover"]
        assert "greeter: ZKit.Greeter" in tip and 'name = "zcode"' in tip, tip
        # The condition held once; the program runs to its end.
        z.run("keys ctrl-cmd-y")
        debug_run_ends(z)

    def ignore_count():
        z.run(f"open {main}:{tick_line}", "wait 300", "action zcode::EditBreakpoint", "wait 300",
              "breakpoint condition", "breakpoint ignore 1", "breakpoint close")
        assert z.dump()["breakpoints"]["list"][0]["ignore"] == 1
        z.run("action zcode::ClearConsole", "keys cmd-r")
        assert "i = 2" in stopped(z)["debugger"]["variables"], "the first hit is ignored"
        z.run("keys ctrl-cmd-y")
        z.wait_for(lambda s: "i = 3" in s["debugger"]["variables"], timeout=60, interval=0.5)
        z.run("keys ctrl-cmd-y")
        debug_run_ends(z)

    def log_message_and_auto_continue():
        z.run(f"open {main}:{tick_line}", "wait 300", "action zcode::EditBreakpoint", "wait 300", "breakpoint ignore 0",
              "breakpoint add-action log tick @i * 10@ in %B, hit %H", "breakpoint add-action command p i + 100",
              "breakpoint auto-continue on", "breakpoint close")
        bp = z.dump()["breakpoints"]["list"][0]
        assert bp["auto_continue"] and [a["type"] for a in bp["actions"]] == ["log", "command"], bp
        z.run("action zcode::ClearConsole", "keys cmd-r")
        tail = debug_run_ends(z)["console_tail"]
        for hit in (1, 2, 3):
            assert f"tick {hit * 10} in main.swift:{tick_line}, hit {hit}" in tail, tail
        assert any(l.endswith("103") for l in tail), tail

    def symbolic_breakpoint():
        z.run("action zcode::DeleteAllBreakpoints", "keys cmd-alt-\\", "wait 300")
        assert z.dump()["breakpoints"]["editor"]["title"] == "Symbolic Breakpoint", z.dump()["breakpoints"]
        z.run("breakpoint symbol Greeter.greeting", "breakpoint module zhello", "breakpoint close")
        assert [b["title"] for b in z.dump()["breakpoints"]["list"]] == ["Greeter.greeting"]
        z.run("action zcode::ClearConsole", "keys cmd-r")
        s = stopped(z)
        assert "Greeter.greeting" in s["debugger"]["frames"][0] and "Greeter.swift:" in s["debugger"]["frames"][0], s["debugger"]
        z.run("keys ctrl-cmd-y")
        debug_run_ends(z)
        # In another module it doesn't resolve, so the run doesn't stop.
        z.run("breakpoint edit 0", "wait 300", "breakpoint module Foundation", "breakpoint close", "action zcode::ClearConsole", "keys cmd-r")
        debug_run_ends(z)

    def swift_error_and_exception_breakpoints():
        z.run("action zcode::DeleteAllBreakpoints", "action zcode::CreateSwiftErrorBreakpoint", "wait 200")
        s = z.dump()
        assert [b["title"] for b in s["breakpoints"]["list"]] == ["Swift Error"] and s["navigator"] == "Breakpoints", s["breakpoints"]
        z.run("action zcode::ClearConsole", "keys cmd-r")
        s = stopped(z)
        assert "Swift Throw" in s["debugger"]["state"], s["debugger"]
        # The throwing line is shown, as in Xcode.
        assert s["active"]["path"] == main and s["active"]["line"] == throw_line, s["active"]
        # An exception breakpoint added while paused reaches lldb at once.
        z.run("action zcode::CreateExceptionBreakpoint", "wait 300", "breakpoint exception Objective-C", "breakpoint close")
        assert [b["title"] for b in z.dump()["breakpoints"]["list"]] == ["Swift Error", "All Objective-C Exceptions"]
        z.run("console breakpoint list")
        listed = "\n".join(console_output(z, "breakpoint list"))
        assert "swift_willThrow" in listed and "objc_exception_throw" in listed, listed
        z.run("keys ctrl-cmd-y")
        debug_run_ends(z)

    def breakpoints_left_for_relaunch():
        # Disabled, so later runs don't stop: checked after a relaunch.
        z.run(f"open {main}:{tick_line}", "wait 300", "keys cmd-\\", "action zcode::EditBreakpoint", "wait 300",
              "breakpoint condition i > 1", "breakpoint add-action sound Pop", "breakpoint close")
        for ix in range(3):
            z.run(f"breakpoint enable {ix} off")
        assert not any(b["enabled"] for b in z.dump()["breakpoints"]["list"])

    check("Breakpoints: gutter click adds, disables and enables; follows inserted lines; double-click edits", breakpoint_gutter)
    check("Breakpoints: condition; LLDB console po/p/expr/v/bt with history; Print Description; data tips", conditional_breakpoint_and_console)
    check("Breakpoints: ignore count", ignore_count)
    check("Breakpoints: Log Message (@expr@, %B, %H) and Debugger Command actions, auto-continue", log_message_and_auto_continue)
    check("Breakpoints: symbolic (⌥⌘\\) with a module", symbolic_breakpoint)
    check("Breakpoints: Swift Error and Objective-C exception breakpoints", swift_error_and_exception_breakpoints)
    check("Breakpoints: leave some for the relaunch check", breakpoints_left_for_relaunch)

    def package_run_with_options():
        z.wait_for(lambda s: s["scheme_options"], timeout=60, interval=0.5)
        z.run("action zcode::RunWithOptions", "wait 300")
        s = scheme_editor(z)
        assert s["perform"] == "Run" and s["tab"] == "Arguments" and s["storage"].startswith("Swift package"), s
        before = z.dump()["reports"]
        z.run("edit-scheme argument --flag \"two words\"", "edit-scheme variable ZHELLO_NAME=Scheme", "edit-scheme perform")
        s = z.wait_for(lambda s: s["reports"] > before and finished(s), timeout=300)
        assert "Hello, Scheme!" in s["console_tail"] and "arguments: --flag two words" in s["console_tail"], s["console_tail"]

    def package_debug_with_options():
        z.run(f"open {main}:{loop_line}", "wait 300", "keys cmd-\\", "keys cmd-r")
        s = z.wait_for(lambda s: s["debugger"]["state"].startswith("Stopped"), timeout=300, interval=0.5)
        # lldb-dap launched it with the scheme's arguments and environment.
        assert "Hello, Scheme!" in s["console_tail"] and "arguments: --flag two words" in s["console_tail"], s["console_tail"]
        z.run(f"open {main}:{loop_line}", "keys cmd-\\", "keys ctrl-cmd-y")
        z.wait_for(finished, timeout=60)

    def package_test_with_options():
        z.run("action zcode::TestWithOptions", "wait 300", "edit-scheme use-run-arguments off",
              "edit-scheme variable ZKIT_TEST_ENV=from-scheme")
        s = run_tests(z, "edit-scheme perform")
        assert "success: true" in s["phase"], s["phase"]
        assert "ZKIT_TEST_ENV=from-scheme" in open(s["last_log"]).read(), "swift test passes the Test action's environment"

    check("Swift package: Run… (⌥⌘R) with arguments and environment kept in the session", package_run_with_options)
    check("Swift package: the debugger (lldb-dap) launches with them too", package_debug_with_options)
    check("Swift package: Test… (⌥⌘U) with the Test action's own environment", package_test_with_options)


def warm_open(samples, background=False, limit_ms=150):
    """Starts a resident zcode (a regular window, or `--background`), then
    times `zcode <project>` through it."""
    socket = os.path.expanduser("~/Library/Caches/zcode/zcode.sock")
    log = os.path.join(WORK, "instance.log")
    env = {k: v for k, v in os.environ.items() if k not in ("ZCODE_DEBUG_DIR",)}
    env["ZCODE_TRACE_STARTUP"] = "1"
    with open(log, "w") as out:
        server = subprocess.Popen([BINARY] + (["--background"] if background else []), env=env, stdout=out, stderr=out)
    try:
        deadline = time.time() + 15
        while not os.path.exists(socket) and time.time() < deadline:
            time.sleep(0.1)
        time.sleep(2.0 if background else 1.0)
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
        assert frame_ms is not None and frame_ms < limit_ms, frame_ms
    finally:
        server.kill()
        server.wait()
        if os.path.exists(socket):
            os.remove(socket)


def compare_apps():
    """Times zcode, Xcode and Zed from launch to their first window on screen
    with scripts/launch_timer.swift, the same external measurement for all
    three. An app that is already running is skipped: it can't be cold
    launched without quitting the user's session."""
    timer = os.path.join(WORK, "launch_timer")
    subprocess.run(["xcrun", "swiftc", "-O", os.path.join(ROOT, "scripts", "launch_timer.swift"), "-o", timer],
                   check=True, capture_output=True)

    def timed(command, runs=3, env=None):
        out = subprocess.run([timer, "--runs", str(runs), "--", *command], capture_output=True, text=True,
                             env=env, timeout=300).stdout
        return json.loads(out.strip().splitlines()[-1])

    def running(pattern):
        return subprocess.run(["pgrep", "-f", pattern], capture_output=True).returncode == 0

    zcode = timed([BINARY], runs=5, env=dict(os.environ, ZCODE_NEW_INSTANCE="1"))
    print(f"      zcode   {zcode['median_ms']:.0f} ms  {zcode['runs_ms']}")
    others = {}
    apps = {
        "Xcode": ("/Applications/Xcode.app/Contents/MacOS/Xcode", ["-ApplePersistenceIgnoreState", "YES"]),
        "Zed": ("/Applications/Zed.app/Contents/MacOS/zed", []),
    }
    for name, (path, args) in apps.items():
        if not os.path.exists(path):
            print(f"      {name:7} not installed")
        elif running(path):
            print(f"      {name:7} skipped: already running")
        else:
            # The first launch warms the OS caches; report the median of the
            # launches after it.
            timed([path, *args], runs=1)
            result = timed([path, *args])
            others[name] = result["median_ms"]
            print(f"      {name:7} {result['median_ms']:.0f} ms  {result['runs_ms']}")
    assert others, "neither Xcode nor Zed could be cold launched"
    for name, ms in others.items():
        assert zcode["median_ms"] < ms, f"{name} ({ms:.0f} ms) launched faster than zcode"


def main():
    if not os.path.exists(BINARY):
        sys.exit("build first: cargo build --release")
    sim = "--simulator" in sys.argv
    print("Cold launch (welcome window; the first launch after a rebuild includes macOS's binary scan):")
    env = dict(os.environ, ZCODE_NEW_INSTANCE="1")

    def median_launch(command):
        subprocess.run(command, capture_output=True, env=env)
        runs = []
        for _ in range(5):
            out = subprocess.run(command, capture_output=True, text=True, env=env).stdout
            runs.append(json.loads(out.strip().splitlines()[-1]))
        median = lambda key: sorted(r[key] for r in runs)[2]
        return median("total_to_first_pixels_ms"), median("total_to_first_frame_ms"), runs[-1]

    zcode_ms, zcode_frame_ms, last = median_launch([BINARY, "--startup-bench"])
    print("  " + json.dumps(last))

    def cold_start():
        # Reference: a minimal native AppKit app (one window, one line of
        # text), launched and timed the same way.
        floor = os.path.join(WORK, "appkit_floor")
        subprocess.run(["xcrun", "swiftc", "-O", os.path.join(ROOT, "scripts", "appkit_floor.swift"), "-o", floor],
                       check=True, capture_output=True)
        native_ms, native_frame_ms, _ = median_launch([floor])
        bare_ms, bare_frame_ms, _ = median_launch([floor, "--no-anim"])
        # "Pixels" is when the window server's copy of the window first shows
        # content, i.e. what the user sees; "frame" is when the app produced
        # its first frame.
        print(f"    median of 5, process start → content on screen (→ first frame):")
        print(f"      zcode {zcode_ms:.0f} ms ({zcode_frame_ms:.0f})")
        print(f"      minimal native AppKit app {native_ms:.0f} ms ({native_frame_ms:.0f})")
        print(f"      same app without the open animation {bare_ms:.0f} ms ({bare_frame_ms:.0f})")
        assert zcode_ms < native_ms, "zcode should show its window before a minimal native app does"

    check("Cold launch: content on screen before a minimal native AppKit app", cold_start)

    if "--compare-apps" in sys.argv:
        check("Cold launch vs Xcode and Zed (first window on screen, timed from outside)", compare_apps)

    samples = os.path.join(WORK, "samples")
    warm_open_check_pending = True
    subprocess.run(["git", "-C", ROOT, "archive", "--format=tar", "-o", os.path.join(WORK, "samples.tar"), "HEAD", "samples"], check=True)
    subprocess.run(["tar", "-xf", "samples.tar"], cwd=WORK, check=True)
    samples = os.path.realpath(samples)

    print("Xcode project (samples/HelloZ):")
    z = Zcode(os.path.join(samples, "HelloZ"), {"ZCODE_PERF_LOG": PERF_LOG})
    try:
        xcode_project_checks(z, samples, sim)
    finally:
        z.quit()

    print("Physical devices (fake devicectl):")
    device_checks(samples)

    print("Group-based Xcode project (samples/HelloClassic):")
    z = Zcode(os.path.join(samples, "HelloClassic"))
    try:
        classic_project_checks(z, samples)
    finally:
        z.quit()

    if warm_open_check_pending:
        print("Single instance:")
        check("Warm open through a running instance (< 150 ms to first frame)", lambda: warm_open(samples))
        check(
            "First open through a `--background` instance (< 200 ms to first frame)",
            lambda: warm_open(samples, background=True, limit_ms=200),
        )

    print("Swift package (samples/ZKit):")
    z = Zcode(os.path.join(samples, "ZKit"))
    try:
        package_checks(z, samples)
    finally:
        z.quit()

    def package_options_persist():
        z = Zcode(os.path.join(samples, "ZKit"))
        try:
            s = z.wait_for(lambda s: s["scheme_options"], timeout=60)
            o = s["scheme_options"]
            assert o["storage"] == "session" and o["run"]["lists"]["environment"] == ["ZHELLO_NAME=Scheme"], o
            assert o["test"]["lists"]["environment"] == ["ZKIT_TEST_ENV=from-scheme"] and not o["test"]["uses_run_arguments"], o
            # Breakpoints come back with their options.
            bps = s["breakpoints"]["list"]
            tick_line = line_of(os.path.join(samples, "ZKit", "Sources", "zhello", "main.swift"), 'print("tick')
            assert [b["title"] for b in bps] == ["Swift Error", "All Objective-C Exceptions", f"main.swift:{tick_line}"], bps
            assert not any(b["enabled"] for b in bps) and bps[2]["condition"] == "i > 1", bps
            assert bps[2]["actions"] == [{"type": "sound", "name": "Pop"}], bps
        finally:
            z.quit()

    check("Swift package: scheme options and breakpoints survive a relaunch (session)", package_options_persist)

    print("Xcode project without scheme files:")

    def schemeless_project():
        project = os.path.join(WORK, "NoSchemes")
        shutil.copytree(os.path.join(samples, "HelloClassic"), project)
        shutil.rmtree(os.path.join(project, "HelloClassic.xcodeproj", "xcshareddata"))
        z = Zcode(project)
        try:
            z.wait_for(lambda s: s["scheme"] == "HelloClassic" and s["scheme_options"], timeout=60)
            z.run("keys cmd-<", "wait 300")
            assert scheme_editor(z)["storage"] == "No scheme file · options kept by zcode", scheme_editor(z)
            z.run("edit-scheme variable GREETING=from-session", "edit-scheme argument -session", "edit-scheme close")
            assert not os.path.exists(os.path.join(project, "HelloClassic.xcodeproj", "xcshareddata")), "no scheme file written"
            z.run("keys cmd-r")
            console_has(z, "arguments: -session GREETING=from-session")
            z.run("keys cmd-.", "wait 1500")
        finally:
            z.quit()

    check("Schemes xcodebuild makes up keep their options in the session; ⌘R uses them", schemeless_project)

    shutil.rmtree(DEBUG_DIR, ignore_errors=True)
    shutil.rmtree(WORK, ignore_errors=True)
    print(f"\n{passed} passed, {failed} failed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

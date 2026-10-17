#!/usr/bin/env python3
"""End-to-end UI checks for zcode, driven through its debug control channel.

    cargo build --release && python3 scripts/e2e.py [--simulator] [--network] [--only <text>]

Launches zcode on the sample projects with ZCODE_DEBUG_DIR set, sends
keystrokes/actions, and asserts on the JSON state from `dump`. `--simulator`
also builds, runs and debugs the iOS sample in a simulator (slower).
Package checks use a local repository of the Tally fixture, so they run
offline; `--network` also lists a public package's versions from GitHub.
`--only <text>[,<text>…]` runs just the checks whose names contain one of them.
"""

import json
import os
import plistlib
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
    ("cmd-alt-w", "zcode::CloseOtherTabs", "File ▸ Close Other Tabs"),
    ("ctrl-cmd-t", "zcode::NewEditorPaneOnRight", "File ▸ New ▸ Editor Pane on Right"),
    ("ctrl-alt-cmd-t", "zcode::NewEditorPaneBelow", "File ▸ New ▸ Editor Pane Below"),
    ("ctrl-shift-cmd-w", "zcode::CloseEditorPane", "File ▸ Close Editor Pane"),
    ("ctrl-alt-shift-cmd-w", "zcode::CloseOtherEditorPanes", "File ▸ Close Other Editor Panes"),
    ("cmd-d", "zcode::DuplicateLine", "Edit ▸ Duplicate"),
    ("ctrl-cmd-'", "zcode::BookmarkLine", "Edit ▸ Bookmark Line"),
    ("ctrl-alt-cmd-'", "zcode::BookmarkLineWithDialog", "Edit ▸ Bookmark Line With Dialog"),
    ("ctrl-shift-cmd-'", "zcode::BookmarkFile", "Edit ▸ Bookmark File"),
    ("ctrl-alt-shift-cmd-'", "zcode::BookmarkFileWithDialog", "Edit ▸ Bookmark File With Dialog"),
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
    ("ctrl-shift-cmd-h", "zcode::FindCallHierarchy", "Find ▸ Find Call Hierarchy"),
    ("ctrl-cmd-j", "zcode::JumpToDefinition", "Navigate ▸ Jump to Definition"),
    ("ctrl-alt-cmd-j", "zcode::JumpToDefinitionInNextPane", "Navigate ▸ Jump to Definition (⌥, in the assistant)"),
    ("ctrl-alt-cmd-up", "zcode::JumpToCounterpartInNextPane", "Navigate ▸ Jump to Next Counterpart (⌥, in the assistant)"),
    ("ctrl-`", "zcode::FocusNextEditorPane", "Navigate ▸ Move Focus to Next Editor Pane"),
    ("ctrl-shift-`", "zcode::FocusPreviousEditorPane", "Navigate ▸ Move Focus to Previous Editor Pane"),
    ("cmd-l", "zcode::JumpToLine", "Navigate ▸ Jump to…"),
    ("ctrl-cmd-left", "zcode::GoBack", "Navigate ▸ Go Back"),
    ("ctrl-cmd-right", "zcode::GoForward", "Navigate ▸ Go Forward"),
    ("ctrl-cmd-up", "zcode::JumpToCounterpart", "Navigate ▸ Jump to Next Counterpart"),
    ("cmd-'", "zcode::NextIssue", "Navigate ▸ Jump to Next Issue"),
    ("cmd-shift-'", "zcode::PreviousIssue", "Navigate ▸ Jump to Previous Issue"),
    ("ctrl-/", "zcode::NextPlaceholder", "Navigate ▸ Jump to Next Placeholder"),
    ("cmd-shift-j", "zcode::RevealInNavigator", "Navigate ▸ Reveal in Project Navigator"),
    ("ctrl-;", "zcode::NextBookmark", "Navigate ▸ Jump to Next Bookmark"),
    ("ctrl-shift-;", "zcode::PreviousBookmark", "Navigate ▸ Jump to Previous Bookmark"),
    ("ctrl-alt-;", "zcode::NextBookmarkInWorkspace", "Navigate ▸ Jump to Next Bookmark In Workspace"),
    ("ctrl-alt-shift-;", "zcode::PreviousBookmarkInWorkspace", "Navigate ▸ Jump to Previous Bookmark In Workspace"),
    ("cmd-shift-[", "zcode::PreviousTab", "Navigate ▸ Show Previous Tab"),
    ("cmd-shift-]", "zcode::NextTab", "Navigate ▸ Show Next Tab"),
    ("cmd-1", "zcode::ShowProjectNavigator", "View ▸ Navigators ▸ Project"),
    ("cmd-2", "zcode::ShowSourceControlNavigator", "View ▸ Navigators ▸ Source Control"),
    ("cmd-3", "zcode::ShowBookmarksNavigator", "View ▸ Navigators ▸ Bookmarks"),
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
    ("ctrl-shift-cmd-enter", "zcode::FocusEditorPane", "View ▸ Editor ▸ Focus Editor Pane"),
    ("cmd-enter", "zcode::ShowEditorOnly", "Editor ▸ Show Editor Only"),
    ("ctrl-alt-cmd-enter", "zcode::ShowAssistant", "Editor ▸ Assistant"),
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
    ("ctrl-\\", "zcode::NextChange", "Navigate ▸ Jump to Next Change"),
    ("ctrl-shift-\\", "zcode::PreviousChange", "Navigate ▸ Jump to Previous Change"),
    ("ctrl-shift-cmd-a", "zcode::ShowAuthors", "Editor ▸ Authors"),
    ("cmd-alt-c", "zcode::Commit", "Integrate ▸ Commit…"),
    ("cmd-alt-x", "zcode::Pull", "Integrate ▸ Pull…"),
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
                    "Choose the team in the target’s Signing & Capabilities tab in the project settings.") in issues, issues
            assert s["navigator"] == "Issues"
            assert "generic/platform=iOS" in open(s["last_log"]).read()
            # Jumping to the issue opens the target's Signing & Capabilities tab.
            z.run("keys cmd-'", "wait 300")
            settings = z.dump()["project_settings"]
            assert settings["shown"] and settings["target"] == "HelloZ" and settings["pane"] == "Signing", settings
            assert "requires a development team" in (settings["signing"]["status"] or ""), settings["signing"]
            z.run("keys cmd-w", "wait 200")

        check("Devices: the destination menu lists devices by OS with their state, Any iOS Device, then simulators", menu)
        check("Devices: plugging and unplugging a device updates the menu", hot_plug)
        check("Devices: Run on an unavailable device says why without building", unavailable_device)
        check("Devices: Run signs, installs, waits for unlock, launches with the scheme's arguments and environment; Stop terminates", run_and_stop)
        check("Devices: Run with breakpoints launches suspended and attaches lldb through CoreDevice", debug_attach)
        check("Devices: a signing error for Any iOS Device shows in the Issue navigator with where to fix it", signing_error)
    finally:
        z.quit()


INFO_PLIST = """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
\t<key>CFBundleDisplayName</key>
\t<string>Hello</string>
\t<key>LSRequiresIPhoneOS</key>
\t<true/>
\t<key>UISupportedInterfaceOrientations</key>
\t<array>
\t\t<string>UIInterfaceOrientationPortrait</string>
\t</array>
</dict>
</plist>
"""


def plist_rows(z):
    return z.dump()["active"]["plist"]


def project_settings_part(z, part):
    return z.dump()["project_settings"][part]


def wait_signing_loaded(z):
    deadline = time.time() + 60
    while not project_settings_part(z, "signing")["loaded"]:
        if time.time() > deadline:
            raise TimeoutError("teams and profiles never loaded")
        time.sleep(0.3)
    return project_settings_part(z, "signing")


def plist_checks(samples):
    """The property list editor, the Info tab and Signing & Capabilities on
    copies of HelloZ (iOS, synchronized folders) and HelloClassic (macOS,
    groups), each built with ⌘B after its capability changes."""
    project = os.path.join(WORK, "PlistZ")
    shutil.copytree(os.path.join(samples, "HelloZ"), project)
    pbxproj = os.path.join(project, "HelloZ.xcodeproj", "project.pbxproj")
    pbxproj_original = open(pbxproj).read()
    # Outside the synchronized folder, so the app doesn't copy them.
    files = os.path.join(project, "Plists")
    os.makedirs(files)
    info = os.path.join(files, "Info.plist")
    with open(info, "w") as f:
        f.write(INFO_PLIST)
    z = Zcode(project)
    try:
        def editor():
            z.run(f"open {info}", "wait 300", "keys cmd-s", "wait 200")
            assert open(info).read() == INFO_PLIST, "saving an unmodified property list is byte for byte"
            p = plist_rows(z)
            assert p["format"] == "Xml" and not p["as_source"], p
            assert p["rows"] == [
                "0|Information Property List|Dictionary|(3 items)",
                "1|Bundle display name (CFBundleDisplayName)|String|Hello",
                "1|Application requires iPhone environment (LSRequiresIPhoneOS)|Boolean|YES",
                "1|Supported interface orientations (UISupportedInterfaceOrientations)|Array|(1 item)",
            ], p["rows"]
            z.run("plist expand UISupportedInterfaceOrientations", "action zcode::ToggleRawKeysAndValues")
            raw = plist_rows(z)["rows"]
            assert raw[0] == "0|Root|Dictionary|(3 items)" and "1|CFBundleDisplayName|String|Hello" in raw, raw
            assert "2|Item 0|String|UIInterfaceOrientationPortrait" in raw, raw
            z.run("action zcode::ToggleRawKeysAndValues")
            assert "2|Item 0|String|Portrait (bottom home button)" in plist_rows(z)["rows"]
            # + on a row adds "New item" after it, its key ready to type;
            # Xcode's names are suggested and pick the key's type.
            z.run("plist add CFBundleDisplayName", "plist type-key New%20item camera usage")
            p = plist_rows(z)
            assert p["editing"] == "camera usage" and p["suggestions"] == ["NSCameraUsageDescription"], p
            z.run("plist pick 0", "plist value NSCameraUsageDescription Scans <codes> & more")
            z.run("plist add NSCameraUsageDescription", "plist type-key New%20item background modes", "plist pick 0")
            z.run("plist expand UIBackgroundModes", "plist add UIBackgroundModes", "plist value UIBackgroundModes/0 fetch")
            z.run("plist type LSRequiresIPhoneOS String", "plist type LSRequiresIPhoneOS Boolean", "plist move LSRequiresIPhoneOS up")
            p = plist_rows(z)
            assert p["rows"][1:6] == [
                "1|Bundle display name (CFBundleDisplayName)|String|Hello",
                "1|Privacy - Camera Usage Description (NSCameraUsageDescription)|String|Scans <codes> & more",
                "1|Application requires iPhone environment (LSRequiresIPhoneOS)|Boolean|YES",
                "1|Required background modes (UIBackgroundModes)|Array|(1 item)",
                "2|Item 0|String|App downloads content from the network",
            ], p["rows"]
            assert z.dump()["active"]["dirty"]
            z.run("keys cmd-s", "wait 200")
            text = open(info).read()
            assert subprocess.run(["plutil", "-lint", info], capture_output=True).returncode == 0, text
            assert ("\t<key>NSCameraUsageDescription</key>\n\t<string>Scans &lt;codes&gt; &amp; more</string>\n"
                    "\t<key>LSRequiresIPhoneOS</key>\n\t<true/>\n\t<key>UIBackgroundModes</key>\n\t<array>\n"
                    "\t\t<string>fetch</string>\n\t</array>\n") in text, text
            # Undo and redo go through the text, so the dirty dot follows.
            z.run("plist remove CFBundleDisplayName")
            assert z.dump()["active"]["dirty"] and "CFBundleDisplayName" not in z.dump()["active"]["text"]
            z.run("keys cmd-z", "wait 100")
            assert not z.dump()["active"]["dirty"], "undo back to the saved text is clean"
            z.run("keys cmd-shift-z", "wait 100")
            assert z.dump()["active"]["dirty"]
            z.run("keys cmd-z", "wait 100", "action zcode::OpenAsSourceCode", "wait 100")
            a = z.dump()["active"]
            assert a["plist"]["as_source"] and a["text"] == text and not a["dirty"], a["plist"]
            z.run("action zcode::OpenAsPropertyList", "wait 100")
            assert not plist_rows(z)["as_source"]

        def binary():
            path = os.path.join(files, "Settings.plist")
            with open(path, "wb") as f:
                plistlib.dump({"zeta": 1, "alpha": [True, "x"]}, f, fmt=plistlib.FMT_BINARY, sort_keys=False)
            z.run(f"open {path}", "wait 300")
            p = plist_rows(z)
            assert p["format"] == "Binary" and p["rows"][:3] == ["0|Root|Dictionary|(2 items)", "1|zeta|Number|1", "1|alpha|Array|(2 items)"], p
            z.run("plist value zeta 2.5", "keys cmd-s", "wait 200")
            data = open(path, "rb").read()
            assert data.startswith(b"bplist00"), data[:8]
            assert list(plistlib.loads(data, dict_type=dict).items()) == [("zeta", 2.5), ("alpha", [True, "x"])]

        def schemas():
            privacy = os.path.join(files, "PrivacyInfo.xcprivacy")
            entitlements = os.path.join(files, "Some.entitlements")
            for path, key in ((privacy, "NSPrivacyTracking"), (entitlements, "com.apple.security.app-sandbox")):
                with open(path, "w") as f:
                    f.write(INFO_PLIST.replace("<key>CFBundleDisplayName</key>\n\t<string>Hello</string>\n\t<key>LSRequiresIPhoneOS</key>",
                                               f"<key>{key}</key>"))
            z.run(f"open {privacy}", "wait 300")
            rows = plist_rows(z)["rows"]
            assert rows[:2] == ["0|App Privacy Configuration|Dictionary|(2 items)", "1|Privacy Tracking Enabled (NSPrivacyTracking)|Boolean|YES"], rows
            z.run(f"open {entitlements}", "wait 300")
            rows = plist_rows(z)["rows"]
            assert rows[:2] == ["0|Entitlements File|Dictionary|(2 items)", "1|App Sandbox (com.apple.security.app-sandbox)|Boolean|YES"], rows

        def info_tab():
            z.run("action zcode::ShowProjectSettings", "wait 300", "project-pane HelloZ Info", "wait 300")
            i = project_settings_part(z, "info")
            assert not i["exists"] and i["path"].endswith("/HelloZ/Info.plist"), i
            assert ("0|Application supports indirect input events (UIApplicationSupportsIndirectInputEvents)|Boolean|YES [build setting]"
                    in i["rows"]), i["rows"]
            z.run("plist setting UIApplicationSupportsIndirectInputEvents NO", "wait 200")
            text = open(pbxproj).read()
            assert text.count("INFOPLIST_KEY_UIApplicationSupportsIndirectInputEvents = NO;") == 2, "both configurations"
            z.run("plist setting UIApplicationSupportsIndirectInputEvents YES", "wait 200")
            assert open(pbxproj).read() == pbxproj_original, "restored byte for byte"
            # The first key creates the target's Info.plist, as Xcode does.
            z.run("plist add /", "plist key New%20item UIRequiresFullScreen", "plist value UIRequiresFullScreen YES", "wait 300")
            i = project_settings_part(z, "info")
            assert i["exists"] and "0|Requires Full Screen (UIRequiresFullScreen)|Boolean|YES" in i["rows"], i
            created = open(os.path.join(project, "HelloZ", "Info.plist")).read()
            assert "\t<key>UIRequiresFullScreen</key>\n\t<true/>\n" in created, created
            text = open(pbxproj).read()
            assert text.count("INFOPLIST_FILE = HelloZ/Info.plist;") == 2, text
            # In a synchronized folder: kept out of the target's resources.
            assert "isa = PBXFileSystemSynchronizedBuildFileExceptionSet;\n\t\t\tmembershipExceptions = (\n\t\t\t\tInfo.plist,\n\t\t\t);" in text, text

        def signing_ios():
            z.run("project-pane HelloZ Signing", "wait 200")
            s = wait_signing_loaded(z)
            assert s["automatic"] and s["bundle"] == "dev.zcode.HelloZ" and s["capabilities"] == [], s
            assert "App Sandbox" not in s["available"] and "Background Modes" in s["available"], s
            assert isinstance(s["teams"], list) and isinstance(s["profiles"], list), s
            z.run("signing add App Groups", "signing list-add App Groups group.dev.zcode.HelloZ", "signing add Push Notifications",
                  "signing add iCloud", "signing toggle iCloud/CloudKit", "signing list-add iCloud iCloud.dev.zcode.HelloZ",
                  "signing add Background Modes", "signing toggle Background Modes/Audio, AirPlay, and Picture in Picture", "wait 300")
            s = project_settings_part(z, "signing")
            assert s["capabilities"] == ["App Groups", "Background Modes", "iCloud", "Push Notifications"], s
            entitlements = plistlib.load(open(os.path.join(project, "HelloZ", "HelloZ.entitlements"), "rb"))
            assert entitlements == {
                "com.apple.security.application-groups": ["group.dev.zcode.HelloZ"],
                "aps-environment": "development",
                "com.apple.developer.icloud-container-identifiers": ["iCloud.dev.zcode.HelloZ"],
                "com.apple.developer.icloud-services": ["CloudKit"],
            }, entitlements
            info_plist = plistlib.load(open(os.path.join(project, "HelloZ", "Info.plist"), "rb"))
            assert info_plist["UIBackgroundModes"] == ["audio"], info_plist
            assert open(pbxproj).read().count("CODE_SIGN_ENTITLEMENTS = HelloZ/HelloZ.entitlements;") == 2
            z.run("keys cmd-w", "wait 200", f"destination {available_simulator_udid()}", "keys cmd-b")
            s = z.wait_for(finished, timeout=600)
            assert "success: true" in s["phase"], (s["phase"], s["issues"])
            assert "HelloZ.app.xcent" in open(s["last_log"]).read(), "signed with the entitlements"
            z.run("action zcode::ShowProjectSettings", "wait 300", "project-pane HelloZ Signing", "wait 200")
            z.run("signing remove App Groups", "signing remove iCloud", "signing remove Push Notifications", "signing remove Background Modes", "wait 300")
            assert project_settings_part(z, "signing")["capabilities"] == []
            assert plistlib.load(open(os.path.join(project, "HelloZ", "HelloZ.entitlements"), "rb")) == {}
            assert "UIBackgroundModes" not in plistlib.load(open(os.path.join(project, "HelloZ", "Info.plist"), "rb"))

        check("Property lists: Xcode's key names, raw keys, suggestions, add/type/move, undo/redo, save, Open As", editor)
        check("Property lists: binary plists open as an outline and save as binary, key order kept", binary)
        check("Property lists: privacy manifests and entitlements get Xcode's names", schemas)
        check("Info tab: INFOPLIST_KEY_ build settings; the first key creates the Info.plist (synchronized exception)", info_tab)
        check("Signing & Capabilities (iOS): App Groups, Push, iCloud, Background Modes written; the app still builds", signing_ios)
    finally:
        z.quit()

    classic = os.path.join(WORK, "PlistClassic")
    shutil.copytree(os.path.join(samples, "HelloClassic"), classic)
    classic_pbxproj = os.path.join(classic, "HelloClassic.xcodeproj", "project.pbxproj")
    z = Zcode(classic)
    try:
        def signing_mac():
            z.run("action zcode::ShowProjectSettings", "wait 300", "project-pane HelloClassic Signing", "wait 200")
            s = wait_signing_loaded(z)
            assert s["certificate"] == "-" and s["status"] is None, s
            assert "App Sandbox" in s["available"] and "Background Modes" not in s["available"], s
            z.run("signing add App Sandbox", "signing toggle App Sandbox/Outgoing Connections (Client)",
                  "signing access App Sandbox/Downloads Folder readwrite", "signing add Hardened Runtime",
                  "signing toggle Hardened Runtime/Allow Execution of JIT-compiled Code", "signing add App Groups",
                  "signing list-add App Groups $(TeamIdentifierPrefix)dev.zcode.HelloClassic", "wait 300")
            text = open(classic_pbxproj).read()
            for line in ("ENABLE_APP_SANDBOX = YES;", "ENABLE_HARDENED_RUNTIME = YES;", "ENABLE_OUTGOING_NETWORK_CONNECTIONS = YES;",
                         "ENABLE_FILE_ACCESS_DOWNLOADS_FOLDER = readwrite;", "ENABLE_USER_SELECTED_FILES = readonly;",
                         "CODE_SIGN_ENTITLEMENTS = HelloClassic/HelloClassic.entitlements;"):
                assert text.count(line) == 2, line
            # A group-based project gets a file reference in the target's group, in no build phase.
            assert "/* HelloClassic.entitlements */ = {isa = PBXFileReference; lastKnownFileType = text.plist.entitlements; path = HelloClassic.entitlements;" in text
            assert "HelloClassic.entitlements in" not in text
            z.run("keys cmd-w", "wait 200", "keys cmd-b")
            s = z.wait_for(finished, timeout=600)
            assert "success: true" in s["phase"], (s["phase"], s["issues"])
            assert "HelloClassic.app.xcent" in open(s["last_log"]).read(), "signed with the entitlements"
            z.run("action zcode::ShowProjectSettings", "wait 300", "project-pane HelloClassic Signing", "wait 200",
                  "signing remove App Sandbox", "wait 300")
            text = open(classic_pbxproj).read()
            assert "ENABLE_APP_SANDBOX" not in text and "ENABLE_OUTGOING_NETWORK_CONNECTIONS" not in text, "the sandbox's settings go"
            assert text.count("ENABLE_HARDENED_RUNTIME = YES;") == 2, "the hardened runtime stays"
            z.run("keys cmd-w")

        check("Signing & Capabilities (macOS): App Sandbox, Hardened Runtime and App Groups; ⌘B signs to run locally", signing_mac)
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
        # Deleting its line (⌘X with nothing selected) deletes it; undo brings
        # the line back and the breakpoint with it, enabled as it was.
        z.run(f"open {main}:{tick_line}", "keys cmd-x", "wait 300")
        assert z.dump()["breakpoints"]["list"] == [], z.dump()["breakpoints"]["list"]
        z.run("keys cmd-z", "wait 300")
        s = z.dump()
        assert [(b["line"], b["enabled"]) for b in s["breakpoints"]["list"]] == [(tick_line, True)], s["breakpoints"]
        assert not s["active"]["dirty"], "undo restored the text"
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

    check("Breakpoints: gutter click adds, disables and enables; follows inserted lines; deleted with its line and back with undo; double-click edits", breakpoint_gutter)
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


def bookmark_rows(z):
    """The Bookmarks navigator's rows as painted: (kind, title) → (x, y)."""
    return {(r["kind"], r["title"]): (r["x"], r["y"]) for r in z.dump()["bookmarks"]["rows"]}


def bookmark_list(s):
    """Every bookmark in the navigator's order as (file, line, title)."""
    return [(b["file"], b["line"], b["title"]) for sec in s["bookmarks"]["sections"] for b in sec["bookmarks"]]


def editor_extension_checks(samples):
    """The editor's extension points (crates/zcode-editor) in the real window,
    through the control channel's `editor` probe, on a copy of ZKit."""
    project = os.path.join(WORK, "EditorKit")
    shutil.copytree(os.path.join(samples, "ZKit"), project, ignore=shutil.ignore_patterns(".build"))
    greeter = os.path.join(project, "Sources", "ZKit", "Greeter.swift")
    with open(greeter) as f:
        text = f.read()
    name_line = line_of(greeter, "public let name")
    init_line = line_of(greeter, "public init")
    after_name = text.index("let name") + len("let name")
    line_start = text.rindex("\n", 0, after_name) + 1
    z = Zcode(project)

    def probe():
        return z.dump()["active"]["probe"]

    def inlays():
        z.run(f"open {greeter}:{name_line}", "wait 400", keys_right(after_name - line_start), "wait 200")
        before = probe()["caret"]
        z.run("keys right", "wait 200")
        plain_step = probe()["caret"]["x"] - before["x"]
        z.run("keys left", f"editor inlay {after_name} after : String", "wait 300")
        s = probe()
        assert s["inlays"] == [[after_name, ": String"]], s
        assert abs(s["caret"]["x"] - before["x"]) < 0.5, "the caret at a type hint's offset stays before it"
        # The caret steps over the inlay in one move; the text didn't change.
        z.run("keys right", "wait 200")
        s = probe()
        assert s["cursor"] == after_name + 1, s
        assert s["caret"]["x"] - before["x"] > plain_step * 6, (s["caret"], before, plain_step)
        assert z.dump()["active"]["text"] == text
        # Typing at its offset keeps the hint after the typed text; undo brings it back.
        z.run("keys left", "type Q", "wait 200")
        assert probe()["inlays"] == [[after_name + 1, ": String"]], probe()
        z.run("keys cmd-z", "wait 200")
        s = z.dump()["active"]
        assert s["probe"]["inlays"] == [[after_name, ": String"]] and not s["dirty"], s
        z.run("screenshot " + os.path.join(WORK, "inlay.png"))

    def blocks():
        rows = dict(map(tuple, probe()["rows"]))
        line_height = rows[init_line + 1] - rows[init_line]
        z.run(f"editor block {name_line - 1} first block line|second block line", "wait 300")
        s = probe()
        assert s["blocks"] == [[name_line - 1, 2]], s
        moved = dict(map(tuple, s["rows"]))
        assert moved[name_line] == rows[name_line], "the block's own row stays"
        assert abs(moved[name_line + 1] - rows[name_line + 1] - 2 * line_height) < 0.5, (moved, rows)
        # The caret moves past the block in one step.
        z.run(f"open {greeter}:{name_line}", "wait 200", "keys down", "wait 200")
        assert z.dump()["active"]["line"] == name_line + 1
        z.run("screenshot " + os.path.join(WORK, "block.png"))

    def highlight_layer():
        z.run(f"editor layer {line_start} {after_name}", "wait 200", f"open {greeter}:1", "type //", "wait 200")
        assert probe()["layer"] == [[line_start + 2, after_name + 2]], probe()
        z.run("keys cmd-z", "wait 200")
        assert probe()["layer"] == [[line_start, after_name]] and not z.dump()["active"]["dirty"]

    def modal():
        z.run("editor clear", f"open {greeter}:{name_line}", "wait 200", "editor modal on", "wait 200")
        assert probe()["cursor_shape"] == "Block"
        # `j` moves instead of typing; `x` deletes; anything else types as usual.
        z.run("keys j", "keys j", "wait 200")
        s = z.dump()["active"]
        assert s["line"] == init_line and s["text"] == text, s["line"]
        z.run("keys x", "type i", "wait 200")
        s = z.dump()["active"]
        assert s["line_text"] == "i" + text.splitlines()[init_line - 1][1:], s["line_text"]
        caret = probe()["caret"]
        assert caret["w"] > 4, "a block caret is a character wide"
        z.run("editor modal off", "wait 200", "keys j", "wait 200")
        assert z.dump()["active"]["line_text"].startswith("ij"), "off again, `j` types"
        for _ in range(4):
            if not z.dump()["active"]["dirty"]:
                break
            z.run("keys cmd-z", "wait 200")
        assert z.dump()["active"]["text"] == text and probe()["cursor_shape"] == "Bar"

    try:
        check("Editor: inlays take room but no offsets; the caret steps over them; they follow typing and undo", inlays)
        check("Editor: blocks push the rows below them down; the caret moves past them", blocks)
        check("Editor: a highlight layer follows edits and undo", highlight_layer)
        check("Editor: a keystroke interceptor runs before the keymap and typing; block caret", modal)
    finally:
        z.quit()


SHAPES_SWIFT = """protocol Shape {
    func area() -> Double
}

class Base: Shape {
    func area() -> Double { 0 }
}

final class Square: Base {
    let side: Double
    init(side: Double) { self.side = side }
    override func area() -> Double { side * side }
}

func total(of shapes: [Shape]) -> Double {
    let areas = shapes.map { $0.area() }
    var sum = 0.0
    for a in areas { sum += a }
    return sum
}

func report() {
    let sq = Square(side: 2)
    let t = total(of: [sq])
    print(t)
}
"""

ADD_C = """int add(int left, int right) { return left + right; }

int main(void) {
    int total = add(1, 2);
    return total;
}
"""


def big_swift(blocks=300):
    """About 6,000 lines of Swift with inferred types, calls and properties."""
    out = []
    for i in range(blocks):
        out.append(f"""struct Item{i} {{
    var name = "item {i}"
    var weight = {i}.5
    func scaled(by factor: Double) -> Double {{
        let base = weight * factor
        let label = name.uppercased()
        let parts = [label, String(base)]
        return parts.isEmpty ? 0 : base
    }}
}}

func use{i}() -> Double {{
    let item = Item{i}()
    let first = item.scaled(by: 2)
    let second = item.scaled(by: first)
    var sum = first + second
    sum += Double(item.name.count)
    return sum
}}

""")
    return "".join(out)


def language_feature_checks(samples):
    """sourcekit-lsp's inlay hints, semantic tokens, signature help and call
    and type hierarchies on a copy of ZKit with a class hierarchy, a loose C
    file (clangd's parameter names) and a 6,000-line Swift file; then the
    same project through a proxy that counts requests, and one that hides
    the features."""
    project = os.path.join(WORK, "LspKit")
    shutil.copytree(os.path.join(samples, "ZKit"), project, ignore=shutil.ignore_patterns(".build"))
    shapes = os.path.join(project, "Sources", "ZKit", "Shapes.swift")
    big = os.path.join(project, "Sources", "ZKit", "Big.swift")
    add_c = os.path.join(project, "tools", "add.c")
    os.makedirs(os.path.dirname(add_c))
    for path, text in [(shapes, SHAPES_SWIFT), (big, big_swift()), (add_c, ADD_C)]:
        with open(path, "w") as f:
            f.write(text)
    proxy = os.path.join(ROOT, "scripts", "lsp_proxy.py")
    traffic = os.path.join(WORK, "lsp-traffic.log")

    def features(z):
        return z.dump()["language_features"]

    def hints_at(z, path, line, timeout=90):
        """Waits for the active editor's hints, opened at `line`."""
        z.run(f"open {path}:{line}")
        return z.wait_for(lambda s: s["language_features"]["inlay_hints"]["active"], timeout=timeout, interval=0.5)[
            "language_features"]["inlay_hints"]["active"]

    z = Zcode(project)
    was_on = features(z)["inlay_hints"]["enabled"]
    shapes_text = SHAPES_SWIFT

    def offset_after(needle):
        return shapes_text.index(needle) + len(needle)

    def server_features():
        z.run(f"open {shapes}:1")
        s = z.wait_for(lambda s: s["lsp"] == "Ready", timeout=120)
        f = s["language_features"]["server"]
        assert f["inlay_hints"] and f["semantic_tokens_full"] and f["call_hierarchy"] and f["type_hierarchy"], f
        assert "(" in f["signature_help"], f

    def inlay_hints():
        z.run("inlay-hints on")
        hints = hints_at(z, shapes, line_of(shapes, "let areas"))
        assert [offset_after("let areas"), ": [Double]"] in hints, hints
        assert [offset_after("let sq"), ": Square"] in hints and [offset_after("for a"), ": Double"] in hints, hints
        z.run("screenshot " + os.path.join(WORK, "inlay-hints.png"))
        # A new binding gets its hint once typing stops; the others move with the text.
        z.run(f"open {shapes}:{line_of(shapes, 'print(t)')}", "keys end", "keys enter", "type let extra = t * 2")
        s = z.wait_for(lambda s: any(h[1] == ": Double" and "extra" in s["active"]["line_text"]
                                     for h in s["language_features"]["inlay_hints"]["active"]
                                     if h[0] > offset_after("print(t)")), timeout=20, interval=0.5)
        hints = s["language_features"]["inlay_hints"]["active"]
        assert [offset_after("let areas"), ": [Double]"] in hints, hints
        z.run("keys cmd-z", "wait 300")
        for _ in range(4):
            if not z.dump()["active"]["dirty"]:
                break
            z.run("keys cmd-z", "wait 200")
        assert not z.dump()["active"]["dirty"]
        # Off: gone from every view.
        z.run("inlay-hints off", "wait 300")
        assert not features(z)["inlay_hints"]["active"], features(z)["inlay_hints"]

    def parameter_names_in_c():
        z.run("inlay-hints on")
        hints = hints_at(z, add_c, 4)
        call = ADD_C.index("add(1") + 4
        assert [call, "left: "] in hints and [call + 3, "right: "] in hints, hints
        z.run("screenshot " + os.path.join(WORK, "parameter-hints.png"))

    def semantic_tokens():
        z.run(f"open {shapes}:1")
        s = z.wait_for(lambda s: s["language_features"]["semantic_tokens"]["count"], timeout=60, interval=0.5)
        tokens = s["language_features"]["semantic_tokens"]["first"]
        # `Square(side: 2)` is a type, not a function call; operators aren't tokens.
        assert "Square" in tokens and "Shape" in tokens and "area" in tokens, tokens
        assert "*" not in tokens and "+=" not in tokens, tokens
        before = s["language_features"]["semantic_tokens"]["count"]
        # Typing keeps them (they follow the edit) and they come back for the new text.
        z.run(f"open {shapes}:{line_of(shapes, 'print(t)')}", "keys end", "keys enter", "type print(sq.side)", "wait 1200")
        assert features(z)["semantic_tokens"]["count"] >= before + 2, features(z)["semantic_tokens"]
        for _ in range(4):
            if not z.dump()["active"]["dirty"]:
                break
            z.run("keys cmd-z", "wait 200")

    def signature_help():
        z.run(f"open {shapes}:{line_of(shapes, 'print(t)')}", "keys end", "keys enter", "type total(")
        s = z.wait_for(lambda s: s["language_features"]["signature_help"], timeout=15, interval=0.3)
        assert s["language_features"]["signature_help"] == "total([of: [any Shape]]) -> Double", s["language_features"]
        z.run("screenshot " + os.path.join(WORK, "signature-help.png"))
        z.run("type [sq])", "wait 800")
        assert features(z)["signature_help"] is None, "closed past the call"
        z.run("type ; _ = Square(", "wait 800")
        assert features(z)["signature_help"] == "init([side: Double])", features(z)["signature_help"]
        z.run("keys escape", "wait 300")
        assert features(z)["signature_help"] is None, "Escape closes it"
        for _ in range(6):
            if not z.dump()["active"]["dirty"]:
                break
            z.run("keys cmd-z", "wait 200")
        assert not z.dump()["active"]["dirty"]

    def hierarchy(timeout=180):
        """Waits for the hierarchy to have a root and its first level."""
        deadline = time.time() + timeout
        while True:
            h = features(z)["hierarchy"]
            if h["rows"] and len(h["rows"]) > 1 and not h["loading"]:
                return h
            if time.time() > deadline:
                raise TimeoutError(h)
            time.sleep(1)

    def call_hierarchy_from_the_focused_pane():
        # Two panes on Shapes.swift; the second, focused one is on `total`.
        z.run(f"open {shapes}:1", "keys ctrl-cmd-t", "wait 400")
        total_line = line_of(shapes, "func total")
        z.run(f"open {shapes}:{total_line}", keys_right(6), "wait 200")
        assert z.dump()["panes"]["focused"] == 1
        # The index may still be building: ask again until it answers, as a user would.
        deadline = time.time() + 180
        while True:
            z.run("keys ctrl-shift-cmd-h", "wait 1500")
            h = features(z)["hierarchy"]
            if h["rows"] and len(h["rows"]) > 1:
                break
            assert time.time() < deadline, h
        h = hierarchy()
        s = z.dump()
        assert s["navigator"] == "Find" and h["shown"] and h["kind"] == "Callers", h
        assert h["rows"] == ["total(of:) — Shapes.swift:15", "  report() — Shapes.swift:24"], h["rows"]
        # A caller opens at its call, in the focused pane.
        z.run("hierarchy open 1", "wait 500")
        s = z.dump()
        assert s["panes"]["focused"] == 1 and s["active"]["line"] == line_of(shapes, "total(of: [sq])"), s["active"]["line"]
        # report() has no callers.
        z.run("hierarchy expand 1", "wait 1500")
        assert hierarchy()["rows"][1:] == ["  report() — Shapes.swift:24"], hierarchy()
        # Callees: what total(of:) calls, the SDK's by module.
        z.run("hierarchy kind callees")
        h = hierarchy()
        assert "  Shape.area() — Shapes.swift:2" in h["rows"] and "  Collection.map(_:) — Swift" in h["rows"], h["rows"]
        z.run("screenshot " + os.path.join(WORK, "call-hierarchy.png"))
        # A callee opens at its declaration.
        row = h["rows"].index("  Shape.area() — Shapes.swift:2")
        z.run(f"hierarchy open {row}", "wait 500")
        assert z.dump()["active"]["line"] == 2
        z.run("keys ctrl-shift-cmd-w", "wait 300")

    def type_hierarchy():
        z.run(f"open {shapes}:{line_of(shapes, 'class Base')}", keys_right(7), "action zcode::FindDescendentTypes")
        h = hierarchy()
        assert h["kind"] == "Subtypes" and h["rows"] == ["Base — Shapes.swift:5", "  Square — Shapes.swift:9"], h
        z.run("hierarchy kind supertypes")
        assert hierarchy()["rows"] == ["Base — Shapes.swift:5", "  Shape — Shapes.swift:1"], hierarchy()
        # Re-rooted on Shape, a protocol: Find Conforming Types.
        z.run(f"open {shapes}:1", keys_right(10), "action zcode::FindConformingTypes")
        h = hierarchy()
        assert h["rows"][:2] == ["Shape — Shapes.swift:1", "  Base — Shapes.swift:5"], h
        z.run("hierarchy expand 1", "wait 1500")
        assert "    Square — Shapes.swift:9" in hierarchy()["rows"], hierarchy()
        z.run("hierarchy close")
        assert not features(z)["hierarchy"]["shown"]

    def typing_latency():
        # In the middle of a 6,000-line file, with hints and tokens on.
        z.run("inlay-hints on", f"open {big}:3000", "wait 300")
        z.wait_for(lambda s: s["language_features"]["semantic_tokens"]["count"] > 1000
                   and s["language_features"]["inlay_hints"]["active"], timeout=120, interval=0.5)
        z.run("keys end", "keys enter", "wait 300")
        on = json.loads(z.run("type-latency let measured = use7() + 1.25 * 3")[-1])
        z.run("inlay-hints off", "wait 1500")
        off = json.loads(z.run("type-latency let measured = use7() + 1.25 * 3")[-1])
        print(f"    keystroke → frame in a 6,000-line file: hints and tokens on {on}, hints off {off}")
        assert on["median_ms"] < 16, on
        for _ in range(80):
            if not z.dump()["active"]["dirty"]:
                break
            z.run("keys cmd-z")

    try:
        check("sourcekit-lsp: what it offers (inlay hints, semantic tokens, signature help, call and type hierarchies)", server_features)
        check("Inlay hints: inferred types after bindings, refreshed after typing, Editor ▸ Inlay Hints off", inlay_hints)
        check("Inlay hints: clangd's parameter names before C arguments", parameter_names_in_c)
        check("Semantic tokens: types at calls, properties and functions over tree-sitter; followed by typing", semantic_tokens)
        check("Signature help: opens on `(`, the argument in brackets, closes after the call and on Escape", signature_help)
        check("Find Call Hierarchy (⌃⇧⌘H) from the focused pane: callers, callees, opening calls and declarations", call_hierarchy_from_the_focused_pane)
        check("Find Ancestor / Descendent / Conforming Types in the Find navigator", type_hierarchy)
        check("Typing in a 6,000-line Swift file with hints and semantic tokens stays under a frame", typing_latency)
    finally:
        z.run("inlay-hints " + ("on" if was_on else "off"))
        z.quit()

    def bounded_traffic():
        if os.path.exists(traffic):
            os.remove(traffic)
        z = Zcode(project, {"ZCODE_SOURCEKIT_LSP": proxy, "ZCODE_LSP_PROXY_LOG": traffic})
        try:
            z.run("inlay-hints on", f"open {big}:3000")
            z.wait_for(lambda s: s["language_features"]["semantic_tokens"]["count"] > 1000
                       and s["language_features"]["inlay_hints"]["active"], timeout=120, interval=0.5)
            z.run("wait 1000")
            before = open(traffic).read().splitlines()
            # 30 keystrokes, then a pause.
            z.run("keys end", "keys enter", "type-latency let typed = use3() + use4() * 2.0", "wait 2000")
            during = open(traffic).read().splitlines()[len(before):]
            count = lambda method: during.count(method)
            print(f"    30 keystrokes: {count('textDocument/semanticTokens/full')} semantic token, "
                  f"{count('textDocument/inlayHint')} inlay hint, {count('textDocument/didChange')} didChange, "
                  f"{count('$/cancelRequest')} cancel requests")
            assert 1 <= count("textDocument/semanticTokens/full") <= 3, during
            assert 1 <= count("textDocument/inlayHint") <= 3, during
            assert count("textDocument/signatureHelp") <= 4, during
            # Scrolling through the file asks for hints, not for every row.
            mark = len(open(traffic).read().splitlines())
            z.run(*[f"open {big}:{line}" for line in range(200, 6000, 400)], "wait 1500")
            scrolled = open(traffic).read().splitlines()[mark:]
            assert scrolled.count("textDocument/inlayHint") <= 16, scrolled.count("textDocument/inlayHint")
            for _ in range(60):
                if not z.dump()["active"]["dirty"]:
                    break
                z.run("keys cmd-z")
        finally:
            z.run("inlay-hints " + ("on" if was_on else "off"))
            z.quit()

    def without_the_features():
        if os.path.exists(traffic):
            os.remove(traffic)
        hidden = "inlayHintProvider,semanticTokensProvider,signatureHelpProvider,callHierarchyProvider,typeHierarchyProvider"
        z = Zcode(project, {"ZCODE_SOURCEKIT_LSP": proxy, "ZCODE_LSP_PROXY_LOG": traffic, "ZCODE_LSP_PROXY_HIDE": hidden})
        try:
            z.run("inlay-hints on", f"open {shapes}:{line_of(shapes, 'func total')}")
            s = z.wait_for(lambda s: s["lsp"] == "Ready", timeout=120)
            assert not any(s["language_features"]["server"].values()), s["language_features"]["server"]
            z.run(keys_right(6), "keys ctrl-shift-cmd-h", "wait 500", f"open {shapes}:{line_of(shapes, 'print(t)')}",
                  "keys end", "keys enter", "type total(", "wait 1500")
            s = z.dump()
            f = s["language_features"]
            assert not f["hierarchy"]["shown"] and f["signature_help"] is None, f
            assert not f["inlay_hints"]["active"] and not f["semantic_tokens"]["count"], f
            sent = open(traffic).read().splitlines()
            for method in ["textDocument/inlayHint", "textDocument/semanticTokens/full", "textDocument/signatureHelp",
                           "textDocument/prepareCallHierarchy"]:
                assert method not in sent, method
            # Completion still works.
            assert "textDocument/completion" in sent
            for _ in range(4):
                if not z.dump()["active"]["dirty"]:
                    break
                z.run("keys cmd-z", "wait 200")
        finally:
            z.run("inlay-hints " + ("on" if was_on else "off"))
            z.quit()

    check("Requests stay bounded: typing and scrolling ask for tokens and hints a few times, not per keystroke", bounded_traffic)
    check("A server without these features: nothing asked for, commands say so, the rest works", without_the_features)


def pane_checks(samples):
    """Editor panes on a copy of ZKit with an Objective-C pair and a property
    list: splits, one buffer in several panes (edits, undo, breakpoints,
    bookmarks, diamonds), the assistant, closing panes, and a relaunch."""
    project = os.path.realpath(os.path.join(WORK, "PaneKit"))
    # Not the package checks' build folder: its paths point at their copy.
    shutil.copytree(os.path.join(samples, "ZKit"), project, ignore=shutil.ignore_patterns(".build"))
    legacy = os.path.join(project, "Legacy")
    os.makedirs(legacy)
    header, implementation = os.path.join(legacy, "Legacy.h"), os.path.join(legacy, "Legacy.m")
    with open(header, "w") as f:
        f.write("int legacy(void);\n")
    with open(implementation, "w") as f:
        f.write('#include "Legacy.h"\n\nint legacy(void) { return 1; }\n')
    info = os.path.join(legacy, "Info.plist")
    with open(info, "w") as f:
        f.write(INFO_PLIST)
    greeter = os.path.join(project, "Sources", "ZKit", "Greeter.swift")
    main = os.path.join(project, "Sources", "zhello", "main.swift")
    tests = os.path.join(project, "Tests", "ZKitTests", "GreeterTests.swift")
    text = open(greeter).read()
    name_line, greeting_line = line_of(greeter, "public let name"), line_of(greeter, "func greeting")
    z = Zcode(project)

    def panes():
        return z.dump()["panes"]

    def shape(p=None):
        p = p or panes()
        return p["layout"], p["focused"], [(x["tabs"], x["active"], x["assistant"]) for x in p["list"]]

    def shown(p, ix):
        return p["list"][ix]["shown"]

    def split_and_one_buffer():
        z.run(f"open {greeter}:{name_line}", "wait 300", "keys ctrl-cmd-t", "wait 400")
        p = panes()
        assert shape(p) == ("h(0, 1)", 1, [(["Greeter.swift"], "Greeter.swift", None)] * 2), shape(p)
        assert shown(p, 1)["line"] == name_line and shown(p, 1)["linked_views"] == 1, shown(p, 1)
        assert z.dump()["buffers"] == ["Greeter.swift"], "two panes, one buffer"
        # Each pane has its own cursor; typing in one shows in the other.
        z.run(f"open {greeter}:{greeting_line}", "wait 200", "type ZZ", "wait 200")
        p = panes()
        assert shown(p, 0)["line"] == name_line and shown(p, 1)["line"] == greeting_line, (shown(p, 0)["line"], shown(p, 1)["line"])
        assert shown(p, 0)["text"] == shown(p, 1)["text"] != text and z.dump()["active"]["dirty"]
        # ⌘Z in the other pane undoes it; ⇧⌘Z back in the first redoes it.
        z.run("pane focus 0", "wait 200", "keys cmd-z", "wait 200")
        p = panes()
        assert shown(p, 0)["text"] == shown(p, 1)["text"] == text and not z.dump()["active"]["dirty"], shown(p, 1)["text"][:80]
        z.run("pane focus 1", "wait 200", "keys cmd-shift-z", "wait 200")
        assert shown(panes(), 0)["text"] != text
        z.run("keys cmd-z", "wait 200", "keys ctrl-alt-cmd-t", "wait 400")
        assert shape()[:2] == ("h(0, v(1, 2))", 2), shape()
        # ⌃` and ⌃⇧` move focus around the panes; a click focuses the one clicked.
        z.run("keys ctrl-`", "wait 200")
        assert panes()["focused"] == 0
        z.run("keys ctrl-shift-`", "wait 200")
        assert panes()["focused"] == 2
        x, y = shown(panes(), 1)["click"]
        z.run(f"click {x} {y}", "wait 300")
        p = panes()
        assert p["focused"] == 1 and z.dump()["active"]["path"] == greeter, p["focused"]
        z.run("screenshot " + os.path.join(WORK, "panes.png"))

    def gutter_in_every_pane():
        # A breakpoint and a bookmark made in one pane show in the others and
        # follow edits and undo made anywhere: one buffer, one set of anchors.
        z.run("pane focus 2", f"open {greeter}:{greeting_line}", "wait 200", "keys cmd-\\", "keys ctrl-cmd-'", "wait 300")
        s = z.dump()
        assert [b["line"] for b in s["breakpoints"]["list"]] == [greeting_line], s["breakpoints"]["list"]
        assert greeting_line in [m["line"] for m in s["breakpoints"]["markers"]], s["breakpoints"]["markers"]
        z.run("pane focus 0", "wait 300")
        s = z.dump()
        assert greeting_line in [m["line"] for m in s["breakpoints"]["markers"]], "the marker shows in every pane"
        assert [g["line"] for g in s["bookmarks"]["gutter"]] == [greeting_line], s["bookmarks"]["gutter"]
        z.run("pane focus 1", f"open {greeter}:1", "wait 200", "keys enter", "wait 300")
        s = z.dump()
        assert [b["line"] for b in s["breakpoints"]["list"]] == [greeting_line + 1], s["breakpoints"]["list"]
        assert [b[1] for b in bookmark_list(s)] == [greeting_line + 1], bookmark_list(s)
        z.run("pane focus 2", "wait 200", "keys cmd-z", "wait 300")
        s = z.dump()
        assert [b["line"] for b in s["breakpoints"]["list"]] == [greeting_line] and not s["active"]["dirty"], s["breakpoints"]["list"]
        # Cut with its line in one pane, back with ⌘Z in another.
        z.run(f"open {greeter}:{greeting_line}", "wait 200", "keys cmd-x", "wait 300")
        assert bookmark_list(z.dump()) == []
        z.run("pane focus 0", "wait 200", "keys cmd-z", "wait 300")
        s = z.dump()
        assert [b["line"] for b in s["breakpoints"]["list"]] == [greeting_line], s["breakpoints"]["list"]
        assert [b[1] for b in bookmark_list(s)] == [greeting_line] and not s["active"]["dirty"], bookmark_list(s)
        z.run(f"open {greeter}:{greeting_line}", "wait 200", "keys cmd-\\", "wait 200")
        assert not z.dump()["breakpoints"]["list"]
        # Test diamonds in a pane split off the test file's.
        z.run("pane focus 2", f"open {tests}", "wait 300", "keys ctrl-cmd-t", "wait 400")
        z.wait_for(lambda s: s["testing"]["gutter"] and s["panes"]["focused"] == 3, timeout=60, interval=0.5)
        z.run("keys ctrl-shift-cmd-w", "wait 300")
        assert shape()[:2] == ("h(0, v(1, 2))", 2), shape()

    def plist_in_two_panes():
        z.run("keys ctrl-alt-shift-cmd-w", "wait 300", f"open {info}", "wait 300", "keys ctrl-cmd-t", "wait 400")
        p = panes()
        assert p["layout"] == "h(0, 1)" and shown(p, 0)["plist"] and shown(p, 1)["plist"], shape(p)
        rows = plist_rows(z)["rows"]
        z.run("plist remove CFBundleDisplayName", "wait 200", "pane focus 0", "wait 300")
        assert [r for r in plist_rows(z)["rows"] if "CFBundleDisplayName" in r] == [], "the edit shows in the other pane's outline"
        z.run("keys cmd-z", "wait 300")
        assert plist_rows(z)["rows"] == rows and not z.dump()["active"]["dirty"]

    def assistant():
        z.run("keys ctrl-alt-shift-cmd-w", "wait 300", f"open {implementation}", "wait 300", "keys ctrl-alt-cmd-enter", "wait 400")
        assert shape()[0] == "h(0, 1)" and shape()[1] == 0, shape()
        assert shape()[2][1] == (["Legacy.h"], "Legacy.h", "Counterparts"), shape()
        # Counterparts follows the editor it assists.
        z.run(f"open {greeter}", "wait 300")
        assert shape()[2][1] == ([], None, "Counterparts"), shape()
        z.run(f"open {header}", "wait 300")
        assert shape()[2][1] == (["Legacy.m"], "Legacy.m", "Counterparts"), shape()
        z.run("screenshot " + os.path.join(WORK, "assistant.png"))
        # Show Editor Only closes it.
        z.run("keys cmd-enter", "wait 300")
        assert shape()[0] == "0", shape()
        # ⌥-Return in Open Quickly opens a Manual assistant; ⌥-click (from the
        # navigator) opens in the assistant with focus in it.
        z.run("keys cmd-shift-o", "type Greeter.swift", "wait 600", "keys alt-enter", "wait 400")
        assert shape()[:2] == ("h(0, 1)", 1) and shape()[2][1] == (["Greeter.swift"], "Greeter.swift", "Manual"), shape()
        z.run(f"pane open-next {implementation}", "wait 300")
        assert shape()[1] == 1 and shape()[2][1] == (["Greeter.swift", "Legacy.m"], "Legacy.m", "Manual"), shape()
        # Next Counterpart with ⌥ (⌃⌥⌘↑) opens it in the assistant.
        z.run("pane focus 0", f"open {header}", "wait 200", "keys ctrl-alt-cmd-up", "wait 300")
        assert shape()[1] == 1 and shape()[2][1][1] == "Legacy.m", shape()

    def definition_in_assistant():
        # A definition in the same file, so it doesn't wait for the package's index.
        call_line = line_of(main, "(try? verify(")
        column = column_of(main, "verify(name)", 2)
        z.run(f"open {main}", "wait 200")
        z.wait_for(lambda s: s["lsp"] == "Ready", timeout=300)
        z.run("pane focus 1", f"open {implementation}", "pane focus 0", "wait 200")
        # The assistant shows the definition; the editor stays where it was.
        # Right after sourcekit-lsp starts its answers can be empty: retry
        # like a user pressing ⌃⌥⌘J again.
        # (Under load, sourcekit-lsp can take minutes to load a fresh package.)
        deadline, tries = time.time() + 240, 0
        while time.time() < deadline:
            tries += 1
            z.run(f"open {main}:{call_line}", "wait 200", keys_right(column), "keys ctrl-alt-cmd-j", "wait 1500")
            p = panes()
            if shown(p, 1)["path"] == main:
                break
        print(f"    answered after {tries} ⌃⌥⌘J")
        assert p["focused"] == 1 and shown(p, 1)["line"] == line_of(main, "func verify"), shape(p)
        assert shown(p, 0)["path"] == main and shown(p, 0)["line"] == call_line, shown(p, 0)

    def close_and_focus():
        # Focus Editor Pane shows the focused pane alone, and toggles back.
        z.run("keys ctrl-shift-cmd-enter", "wait 200")
        assert panes()["zoomed"], shape()
        z.run("keys ctrl-shift-cmd-enter", "wait 200")
        assert not panes()["zoomed"], shape()
        # Close Other Tabs (⌥⌘W) in the focused pane.
        z.run("pane focus 0", "wait 200", "keys cmd-alt-w", "wait 300")
        assert shape()[2][0] == (["main.swift"], "main.swift", None), shape()
        # A file still shown elsewhere closes without asking, dirty or not.
        z.run(f"open {greeter}", "wait 200", "type Q", "keys ctrl-shift-cmd-w", "wait 300")
        p = panes()
        assert shape(p)[:2] == ("0", 0) and "Greeter.swift" in p["list"][0]["tabs"], shape(p)
        # Its edit is still there, in the pane left.
        z.run(f"open {greeter}", "wait 200")
        assert z.dump()["active"]["dirty"], z.dump()["active"]["line_text"]
        z.run("keys cmd-z", "wait 200")
        assert not z.dump()["active"]["dirty"], z.dump()["active"]["line_text"]
        # ⌘W on a split pane's last tab closes the pane.
        z.run("keys ctrl-alt-cmd-t", "wait 300", "keys cmd-w", "wait 300")
        assert shape()[0] == "0", shape()

    def read_only_in_every_pane():
        # A package's checked-out source refuses edits in a pane split off it.
        checkout = os.path.join(project, "Vendor", "SourcePackages", "checkouts", "Tally", "Tally.swift")
        os.makedirs(os.path.dirname(checkout))
        with open(checkout, "w") as f:
            f.write("public let tally = 1\n")
        z.run(f"open {checkout}:1", "wait 300", "keys ctrl-cmd-t", "wait 400", "type x", "keys cmd-/", "wait 200")
        a = z.dump()["active"]
        assert panes()["focused"] == 1 and a["read_only"] and a["editor_read_only"], a
        assert a["text"] == "public let tally = 1\n" and not a["dirty"], a["text"]
        z.run("keys ctrl-shift-cmd-w", "wait 300", "keys cmd-w", "wait 300")
        assert shape()[0] == "0", shape()

    def relaunch():
        nonlocal z
        z.run(f"open {implementation}", "wait 200", "keys ctrl-alt-cmd-enter", "wait 300", "keys ctrl-alt-cmd-t", "wait 400",
              f"open {greeter}:{greeting_line}", "wait 300")
        before = panes()
        assert before["layout"] == "h(v(0, 1), 2)", before["layout"]
        # Drag the divider between the left column and the assistant left (the
        # window's first drag can go to activating it; drag until it moves).
        _, y = shown(before, 2)["click"]
        for _ in range(3):
            divider = shown(panes(), 2)["left"] - 5
            z.run(f"mouse {divider} {y}", "wait 200", f"drag {divider} {y} {divider - 120} {y}", "wait 300")
            before = panes()
            if before["sizes"][0] < before["sizes"][1]:
                break
        width = before["sizes"]
        assert len(width) == 2 and width[1] - width[0] > 200, width
        z.quit()
        z = Zcode(project)
        after = z.wait_for(lambda s: s["panes"]["layout"] != "0", timeout=30)["panes"]
        assert shape(after) == shape(before), (shape(after), shape(before))
        assert abs(after["sizes"][0] - width[0]) < 2, (after["sizes"], width)
        assert shown(after, 1)["path"] == greeter and z.dump()["active"]["path"] == greeter, shown(after, 1)
        assert after["list"][2]["assists"] == 0

    try:
        check("Panes: ⌃⌘T / ⌃⌥⌘T split; two panes are one buffer with a cursor each; undo from either; ⌃` and clicks move focus", split_and_one_buffer)
        check("Panes: breakpoints, bookmarks and test diamonds in every pane, following edits and undo made in any", gutter_in_every_pane)
        check("Panes: a property list in two panes; an edit in one, undone in the other", plist_in_two_panes)
        check("Panes: the assistant's Counterparts follow the editor; ⌘↩; ⌥-Return, ⌥-click and ⌃⌥⌘↑ open in the next pane", assistant)
        check("Panes: ⌃⌥⌘J shows the definition in the assistant", definition_in_assistant)
        check("Panes: Focus Editor Pane, Close Other Tabs, ⌃⇧⌘W without asking for a file shown elsewhere, ⌘W on a pane's last tab", close_and_focus)
        check("Panes: package sources stay read-only in every pane", read_only_in_every_pane)
        check("Panes: the split layout, tabs, assistant and focus come back after a relaunch", relaunch)
    finally:
        z.quit()


def bookmark_checks(samples):
    """Bookmarks on a copy of ZKit, relaunched at the end to check they're kept."""
    project = os.path.join(WORK, "BookmarkKit")
    shutil.copytree(os.path.join(samples, "ZKit"), project, ignore=shutil.ignore_patterns(".build"))
    main = os.path.join(project, "Sources", "zhello", "main.swift")
    greeter = os.path.join(project, "Sources", "ZKit", "Greeter.swift")
    tests = os.path.join(project, "Tests", "ZKitTests", "GreeterTests.swift")
    name_line, loop_line, tick_line = line_of(main, "let name ="), line_of(main, "for i in"), line_of(main, 'print("tick')
    greeting_line = line_of(greeter, "func greeting")
    z = Zcode(project)

    def lines_and_files():
        z.run(f"open {main}:{name_line}", "wait 300", "keys ctrl-cmd-'", f"open {main}:{loop_line}", "wait 300", "keys ctrl-cmd-'",
              "keys ctrl-shift-cmd-'", f"open {greeter}:{greeting_line}", "wait 300", "keys ctrl-cmd-'", "keys cmd-3", "wait 300")
        s = z.dump()
        assert s["navigator"] == "Bookmarks", s["navigator"]
        assert bookmark_list(s) == [
            ("main.swift", name_line, 'let name = ProcessInfo.processInfo.environment["ZHELLO_NAME"] ?? "zcode"'),
            ("main.swift", loop_line, "for i in 1...3 {"),
            ("main.swift", None, "main.swift"),
            ("Greeter.swift", greeting_line, "public func greeting(excited: Bool = false) -> String {"),
        ], bookmark_list(s)
        assert [sec["title"] for sec in s["bookmarks"]["sections"]] == ["main.swift", "Greeter.swift"], s["bookmarks"]["sections"]
        assert [g["line"] for g in s["bookmarks"]["gutter"]] == [greeting_line], s["bookmarks"]["gutter"]
        # Bookmarking a line again reveals its bookmark instead of adding one.
        z.run(f"open {main}:{name_line}", "wait 300", "keys cmd-1", "keys ctrl-cmd-'", "wait 200")
        s = z.dump()
        assert len(bookmark_list(s)) == 4 and s["navigator"] == "Bookmarks", bookmark_list(s)
        assert s["bookmarks"]["selected"].startswith("let name"), s["bookmarks"]["selected"]
        assert [g["line"] for g in s["bookmarks"]["gutter"]] == [name_line, loop_line], s["bookmarks"]["gutter"]
        z.run("screenshot " + os.path.join(WORK, "bookmarks.png"))

    def follow_edits():
        z.run(f"open {main}:1", "keys enter enter", "wait 200")
        lines = [b[1] for b in bookmark_list(z.dump())]
        assert lines == [name_line + 2, loop_line + 2, None, greeting_line], lines
        for _ in range(3):
            if not z.dump()["active"]["dirty"]:
                break
            z.run("keys cmd-z", "wait 200")
        lines = [b[1] for b in bookmark_list(z.dump())]
        assert lines == [name_line, loop_line, None, greeting_line], lines
        # Deleting a bookmarked line deletes its bookmark; undo puts it back
        # in its place in the list.
        z.run(f"open {main}:{loop_line}", "keys cmd-x", "wait 300")
        lines = [b[1] for b in bookmark_list(z.dump())]
        assert lines == [name_line, None, greeting_line], lines
        z.run("keys cmd-z", "wait 300")
        lines = [b[1] for b in bookmark_list(z.dump())]
        assert lines == [name_line, loop_line, None, greeting_line], lines
        assert not z.dump()["active"]["dirty"]

    def next_and_previous():
        z.run(f"open {main}:1", "wait 300", "keys ctrl-;")
        assert z.dump()["active"]["line"] == name_line
        z.run("keys ctrl-;")
        assert z.dump()["active"]["line"] == loop_line
        z.run("keys ctrl-;")
        assert z.dump()["active"]["line"] == name_line, "wraps around the file"
        z.run("keys ctrl-shift-;")
        assert z.dump()["active"]["line"] == loop_line
        # In Workspace, in the navigator's order: main.swift's file bookmark,
        # then Greeter.swift, and back.
        z.run("keys ctrl-alt-;", "wait 300")
        assert z.dump()["active"]["path"].endswith("main.swift")
        z.run("keys ctrl-alt-;", "wait 300")
        s = z.dump()["active"]
        assert s["path"].endswith("Greeter.swift") and s["line"] == greeting_line, s
        z.run("keys ctrl-alt-shift-;", "keys ctrl-alt-shift-;", "wait 300")
        s = z.dump()["active"]
        assert s["path"].endswith("main.swift") and s["line"] == loop_line, s

    def edit_complete_filter():
        z.run("keys cmd-3", "wait 300")
        x, y = bookmark_rows(z)[("bookmark", "for i in 1...3 {")]
        z.run(f"double-click {x} {y}", "wait 300")
        assert z.dump()["bookmarks"]["editing"] == "for i in 1...3 {"
        z.run("keys cmd-a", "type Ticks", "keys enter", "wait 200")
        s = z.dump()
        assert s["bookmarks"]["editing"] is None and bookmark_list(s)[1][2] == "Ticks", bookmark_list(s)
        # Escape leaves the description as it was.
        x, y = bookmark_rows(z)[("bookmark", "Ticks")]
        z.run(f"double-click {x} {y}", "wait 300", "keys cmd-a", "type nope", "keys escape", "wait 200")
        assert bookmark_list(z.dump())[1][2] == "Ticks", bookmark_list(z.dump())
        z.run("bookmark complete 1 on", "bookmark filter tick", "wait 200")
        assert [r[1] for r in bookmark_rows(z)] == ["main.swift", "Ticks"], bookmark_rows(z)
        z.run("bookmark incomplete on", "bookmark filter greet", "wait 200")
        assert [r[1] for r in bookmark_rows(z)] == ["Greeter.swift", "public func greeting(excited: Bool = false) -> String {"], bookmark_rows(z)
        z.run("bookmark filter", "wait 200")
        assert ("bookmark", "Ticks") not in bookmark_rows(z), "completed items hidden"
        z.run("bookmark incomplete off", "wait 200")
        assert len([r for r in bookmark_rows(z) if r[0] == "bookmark"]) == 4

    def drag_and_drop():
        rows = bookmark_rows(z)
        (x1, y1), (x2, y2) = rows[("bookmark", "Ticks")], rows[("bookmark", "main.swift")]
        z.run(f"drag {x1} {y1} {x2} {y2}", "wait 300")
        assert [b[2] for b in bookmark_list(z.dump())][:3] == [bookmark_list(z.dump())[0][2], "main.swift", "Ticks"], bookmark_list(z.dump())
        rows = bookmark_rows(z)
        (x1, y1), (x2, y2) = rows[("section", "Greeter.swift")], rows[("section", "main.swift")]
        z.run(f"drag {x1} {y1} {x2} {y2}", "wait 300")
        s = z.dump()
        assert [sec["title"] for sec in s["bookmarks"]["sections"]] == ["Greeter.swift", "main.swift"], s["bookmarks"]["sections"]
        # Not into another file's bookmarks.
        rows = bookmark_rows(z)
        (x1, y1), (x2, y2) = rows[("bookmark", "Ticks")], rows[("section", "Greeter.swift")]
        z.run(f"drag {x1} {y1} {x2} {y2}", "wait 300")
        assert [sec["title"] for sec in z.dump()["bookmarks"]["sections"]] == ["Greeter.swift", "main.swift"]
        # A click opens the bookmark at its line.
        x, y = bookmark_rows(z)[("bookmark", "Ticks")]
        z.run(f"click {x} {y}", "wait 300")
        s = z.dump()["active"]
        assert s["path"].endswith("main.swift") and s["line"] == loop_line, s

    def dialogs_and_gutter_menu():
        z.run(f"open {main}:{tick_line}", "wait 300", "keys ctrl-alt-cmd-'", "wait 300")
        assert z.dump()["bookmarks"]["dialog"] == {"heading": "Add Line Bookmark", "title": 'print("tick \\(i)")'}, z.dump()["bookmarks"]["dialog"]
        z.run("bookmark title Each tick", "keys enter", "wait 200")
        s = z.dump()
        assert ("main.swift", tick_line, "Each tick") in bookmark_list(s) and not s["modal_open"], bookmark_list(s)
        z.run(f"open {tests}", "wait 300", "keys ctrl-alt-shift-cmd-'", "wait 300")
        assert z.dump()["bookmarks"]["dialog"]["heading"] == "Add File Bookmark"
        z.run("keys escape", "wait 200")
        assert not any(b[0] == "GreeterTests.swift" for b in bookmark_list(z.dump())), "Escape cancels"
        z.run(f"open {main}:{tick_line}", "wait 300")
        ribbon = next(g for g in z.dump()["bookmarks"]["gutter"] if g["line"] == tick_line)
        z.run(f"right-click {ribbon['x']} {ribbon['y']}", "wait 300")
        assert z.dump()["breakpoints"]["menu_open"], "the ribbon's menu"
        assert not z.dump()["breakpoints"]["list"], "a right-click on a ribbon isn't a breakpoint's"
        z.run("keys escape", "wait 200")

    def find_results():
        z.run("keys cmd-shift-f", "wait 200", "keys cmd-a", "type greeting", "keys enter", "wait 1000")
        s = z.dump()
        found = s["find_results"]
        assert found >= 4, (found, s["navigator"], s["files"], s["modal_open"], s["breakpoints"]["menu_open"])
        # With the Find navigator focused, Bookmark Line bookmarks the search.
        z.run("keys ctrl-cmd-'", "wait 300")
        s = z.dump()
        search = next(sec for sec in s["bookmarks"]["sections"] if sec["kind"] == "search")
        assert search["query"] == "greeting" and search["title"] == "“greeting”" and len(search["bookmarks"]) == found, search
        # One result on its own (its context menu's Bookmark Line).
        z.run("bookmark find-result 0 0", "wait 200")
        s = z.dump()
        assert sum(1 for sec in s["bookmarks"]["sections"] for b in sec["bookmarks"]) == 5 + found + 1, bookmark_list(s)
        # Find Again picks up a new match.
        with open(tests, "a") as f:
            f.write("// greeting again\n")
        z.run("bookmark find-again 0")
        z.wait_for(lambda s: len(next(sec for sec in s["bookmarks"]["sections"] if sec["kind"] == "search")["bookmarks"]) == found + 1,
                   timeout=15, interval=0.5)

    def rename_and_trash():
        z.run(f"rename {greeter} -> Greeting.swift", "wait 500")
        s = z.dump()
        assert "Greeting.swift" in [sec["title"] for sec in s["bookmarks"]["sections"]], s["bookmarks"]["sections"]
        assert "Greeter.swift" not in [b[0] for b in bookmark_list(s)], bookmark_list(s)
        renamed = os.path.join(os.path.dirname(greeter), "Greeting.swift")
        z.run(f"trash {renamed}", "wait 500")
        empty_trash_item("Greeting.swift")
        s = z.dump()
        assert "Greeting.swift" not in [b[0] for b in bookmark_list(s)], bookmark_list(s)

    def kept_after_relaunch():
        nonlocal z
        before = bookmark_list(z.dump())
        z.quit()
        z = Zcode(project)
        z.run(f"open {main}", "keys cmd-3", "wait 500")
        s = z.dump()
        assert bookmark_list(s) == before, (bookmark_list(s), before)
        ticks = next(b for sec in s["bookmarks"]["sections"] for b in sec["bookmarks"] if b["title"] == "Ticks")
        assert ticks["completed"], ticks
        # With the Find result's on `greeter.greeting`.
        expected = sorted([name_line, loop_line, tick_line, line_of(main, "greeter.greeting")])
        assert sorted(g["line"] for g in s["bookmarks"]["gutter"]) == expected, s["bookmarks"]["gutter"]
        z.run("bookmark sort name", "wait 200")
        titles = [sec["title"] for sec in z.dump()["bookmarks"]["sections"]]
        assert titles == sorted(titles, key=str.lower), titles
        z.run("bookmark delete-completed", "wait 200")
        assert "Ticks" not in [b[2] for b in bookmark_list(z.dump())]

    try:
        check("Bookmarks: Bookmark Line (⌃⌘') and Bookmark File (⌃⇧⌘'), ribbons, ⌘3, no duplicates", lines_and_files)
        check("Bookmarks: follow inserted lines and undo; a deleted line's bookmark comes back with undo", follow_edits)
        check("Bookmarks: Jump to Next / Previous Bookmark (⌃; ⌃:), In Workspace (⌃⌥; ⌃⌥:)", next_and_previous)
        check("Bookmarks: edit descriptions in place, Mark as Complete, filter, incomplete only", edit_complete_filter)
        check("Bookmarks: drag to reorder bookmarks and files; click to open", drag_and_drop)
        check("Bookmarks: With Dialog variants (⌃⌥⌘', ⌃⌥⇧⌘'); the ribbon's menu", dialogs_and_gutter_menu)
        check("Bookmarks: a Find navigator query and a result; Find Again", find_results)
        check("Bookmarks: follow renames, removed with Move to Trash", rename_and_trash)
        check("Bookmarks: kept after a relaunch; Sort By Name; Delete All Completed Items", kept_after_relaunch)
    finally:
        z.quit()

# git in tests: never the user's global or system configuration (no
# signing, hooks or aliases), and only on temporary repositories.
GIT_ENV = dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1")


def git(cwd, *args):
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True, env=GIT_ENV).stdout


def init_repo(path):
    git(path, "init", "-q", "-b", "main")
    git(path, "config", "user.name", "E2E Author")
    git(path, "config", "user.email", "e2e@example.com")
    git(path, "config", "commit.gpgsign", "false")


def git_checks(samples):
    """Source control on a copy of ZKit made a git repository, with a local
    bare repository as its `origin`: change bars as you type, the hunk
    popover, inline blame and Authors, Code Review, the commit sheet,
    branches, Push, Fetch and Pull, and changes made outside zcode."""
    project = os.path.realpath(os.path.join(WORK, "GitKit"))
    shutil.copytree(os.path.join(samples, "ZKit"), project, ignore=shutil.ignore_patterns(".build"))
    with open(os.path.join(project, ".gitignore"), "w") as f:
        f.write(".build/\n.swiftpm/\n")
    init_repo(project)
    git(project, "add", "-A")
    git(project, "commit", "-q", "-m", "Initial commit")
    origin = os.path.realpath(os.path.join(WORK, "GitKitOrigin.git"))
    git(WORK, "init", "-q", "--bare", "-b", "main", origin)
    git(project, "remote", "add", "origin", origin)
    git(project, "push", "-q", "-u", "origin", "main")
    greeter = os.path.join(project, "Sources", "ZKit", "Greeter.swift")
    rel = "Sources/ZKit/Greeter.swift"
    original = open(greeter).read()
    name_line, init_line = line_of(greeter, "public let name"), line_of(greeter, "public init")
    z = Zcode(project)

    def g(s=None):
        return (s or z.dump())["git"]

    def hunks(s=None):
        return [(h["kind"], h["lines"][0], h["staging"]) for h in g(s)["hunks"]["list"]]

    def settled(s):
        return g(s)["busy"] is None

    def bars_follow_typing():
        z.wait_for(lambda s: g(s)["branch"] == "main" and g(s)["root"] == project, timeout=30)
        z.run(f"open {greeter}:{name_line}", "wait 300")
        z.wait_for(lambda s: g(s)["hunks"] and g(s)["hunks"]["tracked"], timeout=15, interval=0.3)
        assert hunks() == [], hunks()
        # Modify a line, add one, delete one: the bars follow without a save.
        z.run("keys cmd-right", "type  // edited", "wait 300")
        s = z.wait_for(lambda s: hunks(s) == [("Modified", name_line, "Unstaged")], timeout=10, interval=0.2)
        assert not os.path.exists(greeter + ".orig") and open(greeter).read() == original, "nothing saved"
        assert [b["line"] for b in g(s)["hunks"]["bars"]] == [name_line], g(s)["hunks"]["bars"]
        z.run(f"open {greeter}:{init_line}", "keys cmd-right", "keys enter", "type let added = 1", "wait 300")
        z.wait_for(lambda s: ("Added", init_line + 1, "Unstaged") in hunks(s), timeout=10, interval=0.2)
        z.run(f"open {greeter}:1", "keys cmd-x", "wait 300")
        s = z.wait_for(lambda s: hunks(s)[:1] == [("Deleted", 1, "Unstaged")], timeout=10, interval=0.2)
        assert len(hunks(s)) == 3, hunks(s)
        z.run("screenshot " + os.path.join(WORK, "git-bars.png"))

    def popover_discard_and_undo():
        # Click the modified line's bar: the popover shows the change.
        s = z.dump()
        bar = next(b for b in g(s)["hunks"]["bars"] if b["line"] == name_line - 1)
        z.run(f"click {bar['x']} {bar['y']}", "wait 300")
        p = g()["popover"]
        assert p and p["kind"] == "Modified" and p["new"][0].endswith("// edited") and not p["old"][0].endswith("// edited"), p
        z.run("screenshot " + os.path.join(WORK, "git-popover.png"))
        z.run("git popover discard", "wait 300")
        s = z.wait_for(lambda s: [h[0] for h in hunks(s)] == ["Deleted", "Added"], timeout=10, interval=0.2)
        assert "// edited" not in s["active"]["text"] and not s["modal_open"], hunks(s)
        # ⌘Z brings the change back, and its bar.
        z.run("keys cmd-z", "wait 300")
        z.wait_for(lambda s: [h[0] for h in hunks(s)] == ["Deleted", "Modified", "Added"], timeout=10, interval=0.2)
        # Next / Previous Change go from hunk to hunk.
        z.run(f"open {greeter}:1", "keys ctrl-\\", "wait 200")
        assert z.dump()["active"]["line"] == name_line - 1, z.dump()["active"]["line"]
        z.run("keys ctrl-\\", "wait 200")
        assert z.dump()["active"]["line"] == init_line, z.dump()["active"]["line"]
        z.run("keys ctrl-shift-\\", "wait 200")
        assert z.dump()["active"]["line"] == name_line - 1

    def stage_and_unstage_a_hunk():
        # Staged from the popover: the index has the editor's line, drawn hollow.
        z.run("keys cmd-s", "wait 300")
        modified = next(i for i, h in enumerate(hunks()) if h[0] == "Modified")
        bar = next(b for b in g()["hunks"]["bars"] if b["line"] == hunks()[modified][1])
        z.run(f"click {bar['x']} {bar['y']}", "wait 300", "git popover stage")
        s = z.wait_for(lambda s: settled(s) and ("Modified", name_line - 1, "Staged") in hunks(s), timeout=15, interval=0.3)
        cached = git(project, "diff", "--cached")
        assert "// edited" in cached and "let added" not in cached, cached
        assert f"M {rel}" in g(s)["staged"] and f"M {rel}" in g(s)["unstaged"], g(s)
        z.run(f"git unstage-hunk {modified}", "wait 300")
        z.wait_for(lambda s: settled(s) and ("Modified", name_line - 1, "Unstaged") in hunks(s), timeout=15, interval=0.3)
        assert git(project, "diff", "--cached") == ""

    def inline_blame_and_authors():
        z.run("git inline-blame on", f"open {greeter}:{init_line + 3}", "wait 200")
        s = z.wait_for(lambda s: g(s)["blame"]["inline"], timeout=20, interval=0.3)
        inline = g(s)["blame"]["inline"]
        assert inline["line"] == init_line + 3 and inline["text"].startswith("You, ") and inline["text"].endswith("• Initial commit"), inline
        # Typing hides it; it comes back once the blame is read again.
        z.run("type x", "wait 100")
        assert not g()["blame"]["inline"], g()["blame"]
        z.run("keys backspace")
        z.wait_for(lambda s: g(s)["blame"]["inline"] and not g(s)["blame"]["stale"], timeout=20, interval=0.3)
        # Editor ▸ Authors: names in the gutter; edited lines aren't committed yet.
        z.run("keys ctrl-shift-cmd-a", "wait 300")
        s = z.wait_for(lambda s: g(s)["blame"]["painted_authors"], timeout=20, interval=0.3)
        authors = g(s)["blame"]["authors"]
        assert authors[name_line - 2] == "Not Committed Yet" and authors[init_line + 3] == "E2E Author", authors[:12]
        z.run("screenshot " + os.path.join(WORK, "git-authors.png"))
        first = g(s)["blame"]["painted_authors"][0]
        z.run(f"click {first['x']} {first['y']}", "wait 300")
        assert z.dump()["modal_open"]
        z.run("keys escape", "keys ctrl-shift-cmd-a", "wait 300")
        assert not g()["show_authors"] and not g()["blame"]["painted_authors"]

    def code_review():
        z.run("action zcode::SideBySideComparison", f"open {greeter}:{name_line}", "keys alt-shift-cmd-enter", "wait 500")
        s = z.dump()
        assert s["panes"]["list"][0]["active"] == "Greeter.swift (Changes)", s["panes"]["list"][0]
        d = z.wait_for(lambda s: g(s)["diff"] and len(g(s)["diff"]["hunks"]) == 3, timeout=10, interval=0.3)["git"]["diff"]
        assert d["side_by_side"] and d["base_text"] == original, d["hunks"]
        # The added line pads HEAD's side; the deleted one pads the local side.
        assert d["base_blocks"] and d["current_blocks"], d
        z.run("screenshot " + os.path.join(WORK, "git-diff-side-by-side.png"))
        # Typing on the local side edits the file and the comparison follows.
        z.run("keys cmd-down", "type // review", "wait 400")
        d = z.wait_for(lambda s: len(g(s)["diff"]["hunks"]) == 4, timeout=10, interval=0.3)["git"]["diff"]
        assert "Greeter.swift" in z.dump()["buffers"]
        # Inline: deleted lines are blocks under the line before them (the
        # deleted first line, and the modified line's old text).
        z.run("action zcode::InlineComparison", "wait 300")
        d = g()["diff"]
        assert not d["side_by_side"] and d["current_blocks"] == [[1, 1], [1, 1]], d["current_blocks"]
        z.run("screenshot " + os.path.join(WORK, "git-diff-inline.png"))
        z.run("action zcode::SideBySideComparison", "keys cmd-z", "keys alt-shift-cmd-enter", "wait 400")
        s = z.dump()
        assert s["panes"]["list"][0]["active"] == "Greeter.swift" and "// review" not in s["active"]["text"], s["panes"]["list"][0]

    def commit_sheet():
        z.run("keys cmd-alt-c", "wait 500")
        c = z.wait_for(lambda s: g(s)["commit_sheet"] and g(s)["commit_sheet"]["unstaged"] is not None, timeout=15, interval=0.3)["git"]["commit_sheet"]
        assert c["files"] == [f"Unstaged M {rel}"] and len(c["unstaged"]) == 3, c
        assert not c["can_commit"]
        # Stage one hunk, write a message, commit.
        z.run("git commit stage-hunk 1", "wait 300")
        c = z.wait_for(lambda s: settled(s) and len(g(s)["commit_sheet"]["staged"] or []) == 1, timeout=15, interval=0.3)["git"]["commit_sheet"]
        assert c["files"] == [f"Partial M {rel}"] and len(c["unstaged"]) == 2, c
        z.run("type Edit the name line", "wait 200")
        c = g()["commit_sheet"]
        assert c["message"] == "Edit the name line" and c["can_commit"], c
        z.run("screenshot " + os.path.join(WORK, "git-commit.png"))
        z.run("git commit commit", "wait 300")
        z.wait_for(lambda s: settled(s) and g(s)["last_commit"], timeout=20, interval=0.3)
        log = git(project, "log", "--format=%s")
        assert log.splitlines()[0] == "Edit the name line", log
        assert "// edited" in git(project, "show", f"HEAD:{rel}") and "let added" not in git(project, "show", f"HEAD:{rel}")
        # The committed change's bar goes; the others stay.
        z.wait_for(lambda s: [h[0] for h in hunks(s)] == ["Deleted", "Added"], timeout=15, interval=0.3)
        # Amend starts from the last message and keeps one commit.
        z.run("keys cmd-alt-c", "wait 500", "git commit toggle-file " + rel, "wait 300")
        z.wait_for(lambda s: settled(s) and g(s)["commit_sheet"]["files"] == [f"Staged M {rel}"], timeout=15, interval=0.3)
        z.run("git commit amend on", "wait 500")
        c = z.wait_for(lambda s: g(s)["commit_sheet"]["message"] == "Edit the name line", timeout=10, interval=0.3)["git"]["commit_sheet"]
        z.run("git commit message Edit the greeter\\n\\nAmended.", "git commit commit", "wait 300")
        z.wait_for(lambda s: settled(s) and not g(s)["staged"] and not g(s)["unstaged"], timeout=20, interval=0.3)
        assert git(project, "log", "--format=%B", "-1").strip() == "Edit the greeter\n\nAmended."
        assert git(project, "rev-list", "--count", "HEAD").strip() == "2"
        assert hunks() == [], hunks()

    def branches_and_push():
        z.run("action zcode::NewBranch", "wait 300", "type topic", "wait 300", "keys enter")
        z.wait_for(lambda s: settled(s) and g(s)["branch"] == "topic", timeout=15, interval=0.3)
        z.run(f"open {greeter}:{name_line}", "keys cmd-right", "type  // topic", "keys cmd-s", "wait 300",
              "keys cmd-alt-c", "wait 500", "git commit toggle-file " + rel, "wait 500")
        z.wait_for(lambda s: settled(s) and g(s)["commit_sheet"]["files"] == [f"Staged M {rel}"], timeout=15, interval=0.3)
        z.run("git commit message On the topic branch", "git commit commit", "wait 300")
        z.wait_for(lambda s: settled(s) and not g(s)["staged"], timeout=20, interval=0.3)
        # Push… offers a new remote branch named after the local one.
        z.run("action zcode::Push", "wait 300")
        r = g()["remote_sheet"]
        assert r["action"] == "Push" and r["selected"] == "origin/topic" and "origin/main" in r["choices"], r
        z.run("git remote confirm", "wait 300")
        s = z.wait_for(lambda s: settled(s) and g(s)["upstream"] == "origin/topic", timeout=30, interval=0.3)
        assert git(origin, "log", "--format=%s", "-1", "topic").strip() == "On the topic branch"
        # Switch back to main from the picker: the file goes back too.
        z.run("action zcode::SwitchBranch", "wait 300", "type main", "wait 300", "keys enter")
        z.wait_for(lambda s: settled(s) and g(s)["branch"] == "main" and "// topic" not in s["active"]["text"], timeout=20, interval=0.3)
        assert "* main" in g()["branches"] and "topic" in g()["branches"], g()["branches"]

    def fetch_and_pull():
        other = os.path.realpath(os.path.join(WORK, "GitKitOther"))
        git(WORK, "clone", "-q", origin, other)
        git(other, "config", "user.name", "Someone Else")
        git(other, "config", "user.email", "else@example.com")
        git(other, "config", "commit.gpgsign", "false")
        with open(os.path.join(other, "Sources", "ZKit", "Greeter.swift"), "a") as f:
            f.write("// from another clone\n")
        git(other, "commit", "-q", "-am", "From another clone")
        git(other, "push", "-q", "origin", "main")
        z.run("action zcode::FetchChanges", "wait 300")
        z.wait_for(lambda s: settled(s) and g(s)["behind"] == 1, timeout=30, interval=0.3)
        z.run("keys cmd-alt-x", "wait 300")
        r = g()["remote_sheet"]
        assert r["action"] == "Pull" and r["selected"] == "origin/main", r
        z.run("git remote confirm", "wait 300")
        s = z.wait_for(lambda s: settled(s) and g(s)["behind"] == 0 and "// from another clone" in s["active"]["text"], timeout=30, interval=0.3)
        assert not s["active"]["dirty"]
        # Inline blame names the other author's line.
        last = len(s["active"]["text"].splitlines())
        z.run(f"open {greeter}:{last}", "wait 200")
        z.wait_for(lambda s: g(s)["blame"]["inline"] and g(s)["blame"]["inline"]["text"].startswith("Someone Else, "), timeout=20, interval=0.3)

    def outside_changes():
        # Edited and committed in a terminal: the status and bars catch up.
        with open(greeter, "a") as f:
            f.write("// outside\n")
        z.wait_for(lambda s: "// outside" in s["active"]["text"] and hunks(s) == [("Added", len(s["active"]["text"].splitlines()), "Unstaged")],
                   timeout=20, interval=0.5)
        git(project, "commit", "-q", "-am", "Outside zcode")
        z.wait_for(lambda s: hunks(s) == [] and not g(s)["unstaged"], timeout=20, interval=0.5)
        # Discard Changes on a file from the navigator.
        z.run("type // scratch", "keys cmd-s", "wait 300")
        z.wait_for(lambda s: g(s)["unstaged"] == [f"M {rel}"], timeout=10, interval=0.3)
        z.run("keys cmd-2", "wait 300", "screenshot " + os.path.join(WORK, "git-navigator.png"))
        assert z.dump()["navigator"] == "SourceControl"
        z.run(f"git discard {greeter}", "wait 300")
        z.wait_for(lambda s: settled(s) and not g(s)["unstaged"] and "// scratch" not in s["active"]["text"], timeout=15, interval=0.3)
        z.run("keys cmd-1")

    try:
        check("Git: change bars follow typing (modified, added, deleted) against HEAD, before a save", bars_follow_typing)
        check("Git: a bar's popover shows the change; Discard Change, ⌘Z; ⌃\\\\ and ⌃⇧\\\\ step through changes", popover_discard_and_undo)
        check("Git: Stage Change from the popover writes the index (hollow bar); Unstage", stage_and_unstage_a_hunk)
        check("Git: inline blame on the cursor's line hides while typing; Editor ▸ Authors (⌃⇧⌘A) and its commit popover", inline_blame_and_authors)
        check("Git: Code Review (⌥⇧⌘↩) side by side and inline, edited on its local side", code_review)
        check("Git: Commit… (⌥⌘C) with one hunk staged, then Amend", commit_sheet)
        check("Git: New Branch, a commit, Push… to a new remote branch, Switch Branch back", branches_and_push)
        check("Git: Fetch Changes and Pull… (⌥⌘X) from a local origin; the open file reloads", fetch_and_pull)
        check("Git: a commit made outside zcode clears the bars; Discard Changes on a file", outside_changes)
    finally:
        z.quit()


def tally_repo():
    """The Tally fixture package as a bare git repository with release tags."""
    out = subprocess.run(["sh", os.path.join(ROOT, "tests", "fixtures", "packages", "make_repo.sh"), os.path.join(WORK, "repos")],
                         capture_output=True, text=True, check=True)
    return out.stdout.strip()


def tag_release(repo, tag):
    """Publishes a new release of the fixture repository."""
    clone = os.path.join(WORK, f"tally-clone-{tag}")
    git = ["git", "-c", "user.name=t", "-c", "user.email=t@t.invalid"]
    subprocess.run(git + ["clone", "-q", repo, clone], check=True)
    subprocess.run(git + ["commit", "-q", "--allow-empty", "-m", tag], cwd=clone, check=True)
    subprocess.run(git + ["tag", tag], cwd=clone, check=True)
    subprocess.run(git + ["push", "-q", "origin", "main", tag], cwd=clone, check=True)


def packages_done(z, jobs, timeout=300):
    """Waits until `jobs` package jobs have finished and none is running."""
    return z.wait_for(lambda s: s["packages"]["completed"] >= jobs and not s["packages"]["busy"], timeout=timeout, interval=0.5)


def add_package_sheet(z, url, *fields, products=()):
    """Add Package Dependencies for `url`: rule `fields`, then the products step."""
    z.run(f"package open {url}")
    s = z.wait_for(lambda s: (s["packages"]["add_package"] or {}).get("versions") is not None
                   or (s["packages"]["add_package"] or {}).get("error"), timeout=60, interval=0.3)
    sheet = s["packages"]["add_package"]
    assert not sheet["error"], sheet
    z.run(*[f"package {f}" for f in fields], "package next")
    s = z.wait_for(lambda s: (s["packages"]["add_package"] or {}).get("step") == "Products", timeout=120, interval=0.3)
    z.run(*[f"package target {p}" for p in products])
    return sheet, z.dump()["packages"]["add_package"]


def package_dependency_checks(samples):
    repo = tally_repo()
    url = f"file://{repo}"
    root = os.path.join(WORK, "Packages")
    os.makedirs(root)
    shutil.copytree(os.path.join(samples, "HelloClassic"), os.path.join(root, "HelloClassic"))
    shutil.copytree(os.path.join(samples, "ZKit"), os.path.join(root, "ZKit"), ignore=shutil.ignore_patterns(".build"))
    shutil.copytree(os.path.join(samples, "ZKit"), os.path.join(root, "LocalKit"), ignore=shutil.ignore_patterns(".build"))
    classic = os.path.join(root, "HelloClassic")
    pbxproj = os.path.join(classic, "HelloClassic.xcodeproj", "project.pbxproj")
    resolved = os.path.join(classic, "HelloClassic.xcodeproj", "project.xcworkspace", "xcshareddata", "swiftpm", "Package.resolved")
    uses = os.path.join(classic, "HelloClassic", "UsesPackages.swift")

    def pins():
        # xcodebuild deletes Package.resolved once nothing is left to pin.
        if not os.path.exists(resolved):
            return {}
        return {p["identity"]: p["state"].get("version") for p in json.load(open(resolved))["pins"]}


    z = Zcode(classic)
    try:
        def add_remote():
            z.wait_for(lambda s: s["xcode_projects"] and s["scheme"], timeout=60, interval=0.3)
            before = open(pbxproj).read()
            sheet, products = add_package_sheet(z, url, products=["TallyFormat None"])
            assert sheet["versions"] == ["2.0.0", "1.2.0", "1.1.0", "1.0.0"] and sheet["branches"] == ["main", "develop"], sheet
            assert sheet["rule"] == "Up to Next Major Version" and sheet["resolves_to"] == "2.0.0", sheet
            assert products["products"] == ["Tally: Library → HelloClassic", "TallyFormat: Library → None"], products
            z.run("package add")
            s = packages_done(z, 1)
            assert not s["packages"]["issues"], s["packages"]["issues"]
            assert s["packages"]["list"] == ["Tally 2.0.0 direct checkout"], s["packages"]
            assert s["packages"]["tree"] == ["Package Dependencies", "  Tally"], s["packages"]["tree"]
            text = open(pbxproj).read()
            assert lines_kept(before, text), "only inserted lines"
            assert 'XCRemoteSwiftPackageReference "Tally"' in text and "kind = upToNextMajorVersion;" in text, text
            assert "/* Tally in Frameworks */ = {isa = PBXBuildFile; productRef = " in text and "TallyFormat" not in text
            assert pins() == {"tally": "2.0.0"}, pins()

        def add_local():
            z.run(f"package local {os.path.join(root, 'LocalKit')}")
            s = z.wait_for(lambda s: (s["packages"]["add_package"] or {}).get("step") == "Products", timeout=60, interval=0.3)
            assert s["packages"]["add_package"]["products"] == ["ZKit: Library → HelloClassic", "zhello: Executable → None"], s["packages"]
            z.run("package add")
            s = packages_done(z, 2)
            assert "LocalKit local direct checkout" in s["packages"]["list"], s["packages"]
            assert 'relativePath = ../LocalKit;' in open(pbxproj).read()

        def builds_with_packages():
            with open(uses, "w") as f:
                f.write("import Tally\nimport ZKit\n\nfunc tallied() -> String {\n    var tally = Tally()\n    tally.add()\n"
                        "    return Tally.version + ZKit.Greeter(name: \"\\(tally.count)\").greeting()\n}\n")
            z.run(f"new-file {uses}")
            wait_file(pbxproj, lambda t: "UsesPackages.swift in Sources" in t)
            z.run("keys cmd-b")
            s = z.wait_for(lambda s: s["phase"].startswith("Finished"), timeout=400)
            assert "success: true" in s["phase"], (s["phase"], s["issues"][:5])

        def browse_and_jump_read_only():
            checkout = z.dump()["packages"]["checkouts"]["tally"]
            source = os.path.join(checkout, "Sources", "Tally", "Tally.swift")
            z.run(f"reveal {source}", "wait 300")
            rows = z.dump()["packages"]["tree"]
            assert rows[:5] == ["Package Dependencies", "  LocalKit", "  Tally", "    Sources", "      Tally"] and "        Tally.swift" in rows, rows
            # Jump to Definition from the app's code lands in the package,
            # read-only, once the index knows the package.
            z.wait_for(lambda s: s["lsp"] == "Ready" and not s["indexing"], timeout=400, interval=1)
            line = line_of(uses, "var tally = Tally()")
            z.run(f"open {uses}:{line}", keys_right(column_of(uses, "Tally()", 1)), "wait 1500")
            s = None
            for _ in range(20):
                z.run("keys ctrl-cmd-j", "wait 1500")
                s = z.dump()
                if s["active"]["path"].endswith("Tally.swift"):
                    break
            assert "/checkouts/Tally/Sources/Tally/Tally.swift" in s["active"]["path"], s["active"]["path"]
            assert s["active"]["read_only"] and s["active"]["editor_read_only"], s["active"]
            text = s["active"]["text"]
            z.run("type zzz", "keys cmd-/", "keys cmd-d", "keys cmd-alt-]", "keys cmd-z", "keys cmd-s", "wait 300")
            s = z.dump()
            assert not s["active"]["dirty"] and "zzz" not in open(s["active"]["path"]).read(), "package sources aren't edited"
            # The editor itself refuses the editing commands (comment, duplicate, move line) and undo.
            assert s["active"]["text"] == text and not s["active"]["can_undo"], s["active"]

        def update_package():
            tag_release(repo, "2.1.0")
            z.run("action zcode::ResolvePackageVersions")
            s = packages_done(z, 3)
            assert pins() == {"tally": "2.0.0"}, "resolving keeps the pinned version"
            z.run("package-menu update tally")
            s = packages_done(z, 4)
            assert pins() == {"tally": "2.1.0"}, pins()
            assert "Tally 2.1.0 direct checkout" in s["packages"]["list"], s["packages"]["list"]

        def resolve_error_in_issue_navigator():
            shutil.move(repo, repo + ".away")
            try:
                z.run("action zcode::ResetPackageCaches")
                s = packages_done(z, 5)
                assert s["packages"]["issues"] and "Tally.git" in s["packages"]["issues"][0], s["packages"]
                assert s["navigator"] == "Issues", s["navigator"]
            finally:
                shutil.move(repo + ".away", repo)
            z.run("action zcode::ResolvePackageVersions")
            s = packages_done(z, 6)
            assert not s["packages"]["issues"] and "Tally 2.1.0 direct checkout" in s["packages"]["list"], s["packages"]

        def remove_packages():
            z.run("package-menu remove tally")
            s = packages_done(z, 7)
            z.run("package-menu remove localkit")
            s = packages_done(z, 8)
            assert s["packages"]["list"] == [] and s["packages"]["tree"] == [], s["packages"]
            text = open(pbxproj).read()
            assert "Tally" not in text and "LocalKit" not in text and "productRef = " not in text, text
            assert pins() == {}, pins()

        check("Packages: Add Package Dependencies lists tags, adds a product to a target, resolves", add_remote)
        check("Packages: Add Local… adds a package folder", add_local)
        check("Packages: the app builds against both packages (⌘B)", builds_with_packages)
        check("Packages: browse a package's sources; Jump to Definition opens them read-only", browse_and_jump_read_only)
        check("Packages: Resolve keeps the pin; Update Package moves to a new release", update_package)
        check("Packages: a resolve error shows under Package Loading", resolve_error_in_issue_navigator)
        check("Packages: Remove Package takes packages out of project.pbxproj", remove_packages)
    finally:
        z.quit()

    kit = os.path.join(root, "ZKit")
    manifest = os.path.join(kit, "Package.swift")
    z = Zcode(kit)
    try:
        def swift_package_add():
            z.wait_for(lambda s: s["scheme"], timeout=60)
            _, products = add_package_sheet(z, url, "rule minor", "version 1.1.0", products=["TallyFormat None"])
            assert products["targets"] == ["ZKit", "zhello", "ZKitTests"], products
            z.run("package add")
            s = packages_done(z, 1)
            assert not s["packages"]["issues"], s["packages"]["issues"]
            assert s["packages"]["list"] == ["Tally 1.1.0 direct checkout"] and s["packages"]["rules"] == ["Tally 1.1.0 – Next Minor"], s["packages"]
            text = open(manifest).read()
            assert f'.package(url: "{url}"' in text and '.product(name: "Tally", package: "Tally")' in text, text
            assert json.load(open(os.path.join(kit, "Package.resolved")))["pins"][0]["state"]["version"] == "1.1.0"

        def swift_package_builds_and_removes():
            greeter = os.path.join(kit, "Sources", "ZKit", "Greeter.swift")
            with open(greeter, "a") as f:
                f.write("\nimport Tally\n\npublic func tallyVersion() -> String { Tally.version }\n")
            z.run("keys cmd-b")
            s = z.wait_for(lambda s: s["phase"].startswith("Finished"), timeout=400)
            assert "success: true" in s["phase"], (s["phase"], s["issues"][:5])
            with open(greeter) as f:
                restored = f.read().split("\nimport Tally\n")[0]
            with open(greeter, "w") as f:
                f.write(restored)
            z.run("package-menu remove tally")
            s = packages_done(z, 2)
            assert s["packages"]["list"] == [], s["packages"]
            assert "Tally" not in open(manifest).read()

        check("Swift package: Add Package Dependencies writes Package.swift (swift package add-dependency) and resolves", swift_package_add)
        check("Swift package: builds with the package; Remove Package edits Package.swift", swift_package_builds_and_removes)
    finally:
        z.quit()

    def workspace_packages():
        ws_root = os.path.join(WORK, "PackagesWorkspace")
        shutil.copytree(os.path.join(samples, "HelloClassic"), ws_root)
        workspace = os.path.join(ws_root, "Classic.xcworkspace")
        os.makedirs(workspace)
        with open(os.path.join(workspace, "contents.xcworkspacedata"), "w") as f:
            f.write('<?xml version="1.0" encoding="UTF-8"?>\n<Workspace\n   version = "1.0">\n   <FileRef\n'
                    '      location = "group:HelloClassic.xcodeproj">\n   </FileRef>\n</Workspace>\n')
        z = Zcode(workspace)
        try:
            z.wait_for(lambda s: s["xcode_projects"] and s["scheme"], timeout=60, interval=0.3)
            add_package_sheet(z, url, "rule exact", "version 1.2.0")
            z.run("package add")
            s = packages_done(z, 1)
            assert not s["packages"]["issues"] and s["packages"]["list"] == ["Tally 1.2.0 direct checkout"], s["packages"]
            pinned = json.load(open(os.path.join(workspace, "xcshareddata", "swiftpm", "Package.resolved")))["pins"]
            assert pinned[0]["state"]["version"] == "1.2.0", pinned
            assert 'XCRemoteSwiftPackageReference "Tally"' in open(os.path.join(ws_root, "HelloClassic.xcodeproj", "project.pbxproj")).read()
        finally:
            z.quit()

    check("Packages in an .xcworkspace: added to its project, resolved for the workspace", workspace_packages)

    if "--network" in sys.argv:
        def public_package_versions():
            z = Zcode(os.path.join(root, "ZKit"))
            try:
                z.run("package open apple/swift-argument-parser")
                s = z.wait_for(lambda s: (s["packages"]["add_package"] or {}).get("versions"), timeout=60, interval=0.5)
                sheet = s["packages"]["add_package"]
                assert sheet["url"] == "https://github.com/apple/swift-argument-parser" and "1.0.0" in sheet["versions"], sheet
                z.run("package cancel")
            finally:
                z.quit()

        check("Packages (network): versions of github.com/apple/swift-argument-parser", public_package_versions)


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

    print("Property lists, Info and Signing & Capabilities (copies of HelloZ and HelloClassic):")
    plist_checks(samples)

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

    print("Bookmarks (a copy of samples/ZKit):")
    bookmark_checks(samples)

    print("Editor extension points (a copy of samples/ZKit):")
    editor_extension_checks(samples)

    print("Language features (a copy of samples/ZKit, sourcekit-lsp):")
    language_feature_checks(samples)

    print("Editor panes (a copy of samples/ZKit):")
    pane_checks(samples)

    print("Source control (a copy of samples/ZKit in a new repository, with a local origin):")
    git_checks(samples)

    print("Package dependencies (offline, a local repository of the Tally fixture):")
    package_dependency_checks(samples)

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

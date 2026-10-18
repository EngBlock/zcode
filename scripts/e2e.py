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

import base64
import json
import os
import plistlib
import socket
import stat
import statistics
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "scripts"))
SAMPLES = os.path.normpath(os.path.join(ROOT, "samples"))
# Tests run against a private copy so edits, breakpoints and session state from
# other work in the repo can't leak in (and test edits never touch the repo).
WORK = tempfile.mkdtemp(prefix="zcode-e2e-samples-")
PERF_LOG = os.path.join(WORK, "perf.jsonl")
# The zcode under test: this checkout's release build, or `ZCODE_E2E_BINARY`
# (another build, for a comparison).
BINARY = os.environ.get("ZCODE_E2E_BINARY") or os.path.join(ROOT, "target", "release", "zcode")
DEBUG_DIR = tempfile.mkdtemp(prefix="zcode-e2e-")
# Settings, key bindings, themes and the session live in a folder of the
# run's own (ZCODE_CONFIG_DIR), so the suite never reads or changes the
# user's ~/.config/zcode or their session; every zcode it starts inherits it.
CONFIG_DIR = tempfile.mkdtemp(prefix="zcode-e2e-config-")
os.environ["ZCODE_CONFIG_DIR"] = CONFIG_DIR
# Nothing opens Xcode on the screen: Open in Xcode and Show in Xcode Canvas
# record their commands here instead (src/xcode_app.rs).
XCODE_COMMANDS = os.path.join(CONFIG_DIR, "xcode-commands.jsonl")
os.environ["ZCODE_XCODE_COMMANDS"] = XCODE_COMMANDS

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
    ("ctrl-alt-space", "zcode::ShowPredictiveCompletion", "Editor ▸ Show Predictive Completion"),
    ("cmd-+", "zcode::IncreaseFontSize", "Editor ▸ Font Size ▸ Increase"),
    ("cmd--", "zcode::DecreaseFontSize", "Editor ▸ Font Size ▸ Decrease"),
    ("ctrl-cmd-0", "zcode::ResetFontSize", "Editor ▸ Font Size ▸ Reset"),
    ("ctrl-shift-cmd-l", "zcode::ToggleSoftWrap", "Editor ▸ Wrap Lines"),
    ("cmd-f", "zcode::FindInFile", "Find ▸ Find…"),
    ("cmd-alt-f", "zcode::FindAndReplaceInFile", "Find ▸ Find and Replace…"),
    ("cmd-alt-e", "zcode::SelectNextOccurrence", "Find ▸ Select Next Occurrence"),
    ("cmd-shift-f", "zcode::FindInWorkspace", "Find ▸ Find in Workspace…"),
    ("alt-shift-cmd-f", "zcode::FindAndReplaceInWorkspace", "Find ▸ Find and Replace in Workspace…"),
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
    ("cmd-0", "zcode::ShowCodingAssistant", "View ▸ Navigators ▸ Show Coding Assistant"),
    ("ctrl-cmd-h", "zcode::NewConversation", "File ▸ New ▸ Conversation"),
    ("cmd-shift-y", "zcode::ToggleDebugArea", "View ▸ Debug Area ▸ Show Debug Area"),
    ("cmd-alt-1", "zcode::ShowFileInspector", "View ▸ Inspectors ▸ File"),
    ("cmd-shift-c", "zcode::ActivateConsole", "View ▸ Debug Area ▸ Activate Console"),
    ("ctrl-6", "zcode::ShowDocumentItems", "View ▸ Editor ▸ Document Items"),
    ("ctrl-shift-cmd-enter", "zcode::FocusEditorPane", "View ▸ Editor ▸ Focus Editor Pane"),
    ("alt-cmd-enter", "zcode::ShowInXcodeCanvas", "Editor ▸ Canvas (zcode: Show in Xcode Canvas)"),
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
    ("cmd-i", "zcode::Profile", "Product ▸ Profile"),
    ("cmd-alt-i", "zcode::ProfileWithOptions", "Product ▸ Profile…"),
    ("ctrl-cmd-i", "zcode::ProfileWithoutBuilding", "Product ▸ Perform Action ▸ Profile Without Building"),
    ("cmd-shift-i", "zcode::BuildForProfiling", "Product ▸ Build For ▸ Profiling"),
    ("alt-shift-cmd-o", "zcode::ShowOrganizer", "Window ▸ Organizer"),
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
    def __init__(self, project, extra_env=None, keep_vim=False, binary=BINARY):
        # The debug log is shared by every instance: read only this one's
        # lines, so an earlier check's errors can't fail this launch.
        try:
            with open(os.path.join(DEBUG_DIR, "log")) as f:
                self.log_offset = len(f.read().splitlines())
        except FileNotFoundError:
            self.log_offset = 0
        env = dict(os.environ, ZCODE_DEBUG_DIR=DEBUG_DIR, **(extra_env or {}))
        # zcode's own log, for a check that fails: $ZCODE_E2E_STDERR.
        stderr = open(os.environ["ZCODE_E2E_STDERR"], "a") if os.environ.get("ZCODE_E2E_STDERR") else subprocess.DEVNULL
        self.proc = subprocess.Popen([binary, project], env=env,
                                     stdout=subprocess.DEVNULL, stderr=stderr)
        self.run("wait 1")
        # The run's settings are shared by its checks: Vim mode is off
        # unless a check turns it on.
        if not keep_vim:
            self.run("vim off")

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
                raise RuntimeError(f"zcode exited (status {self.proc.returncode})")
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
        raise TimeoutError("condition not met; last state: " + json.dumps(state)[:int(os.environ.get("E2E_STATE_CHARS", "600"))])

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
        if os.environ.get("E2E_TRACE"):
            import traceback
            traceback.print_exc()


def keys_right(n):
    return "keys " + " ".join(["right"] * n)


def replay_lsp_changes(log, suffix):
    """The text a language server has for the file whose path ends with
    `suffix`, from the didOpen and didChange params `lsp_proxy.py` logged
    (`ZCODE_LSP_PROXY_CHANGES`), and the content changes sent."""
    def offset(text, position):
        lines = text.splitlines(keepends=True)
        start = sum(len(line) for line in lines[:position["line"]])
        line = lines[position["line"]] if position["line"] < len(lines) else ""
        units = 0
        for i, ch in enumerate(line):
            if units >= position["character"]:
                return start + i
            units += 2 if ord(ch) > 0xFFFF else 1
        return start + len(line)

    text, changes = None, []
    for entry in open(log):
        message = json.loads(entry)
        document = message["params"]["textDocument"]
        if not document["uri"].endswith(suffix):
            continue
        if message["method"] == "textDocument/didOpen":
            text, changes = document["text"], []
            continue
        for change in message["params"]["contentChanges"]:
            changes.append(change)
            if "range" not in change:
                text = change["text"]
                continue
            start, end = offset(text, change["range"]["start"]), offset(text, change["range"]["end"])
            text = text[:start] + change["text"] + text[end:]
    return text, changes


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


def process_gone(pid, timeout=15):
    """Waits for a process to be gone (not even a zombie)."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return True
        time.sleep(0.2)
    out = subprocess.run(["ps", "-o", "pid,ppid,pgid,stat,comm", "-p", str(pid)], capture_output=True, text=True).stdout
    raise AssertionError(f"process {pid} is still there:\n{out}")


def running_app_pid(z, timeout=60):
    """The pid of the program zcode is running (or debugging) on this Mac."""
    pid = z.wait_for(lambda s: s["run_pid"], timeout=timeout, interval=0.3)["run_pid"]
    os.kill(pid, 0)
    return pid


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
        # A never-indexed project runs a background index build, whose index
        # sourcekit-lsp reads as it's written; wait for that too.
        z.wait_for(lambda s: s["lsp"] == "Ready" and not s["issues"] and not s["indexing"], timeout=300)
        assert z.dump()["build_server"]["through_build_server"], "sourcekit-lsp gets its arguments from zcode's build server"

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

    def mcp_simulator_screenshot():
        # zcode's MCP server (this instance listens in the run's own
        # configuration folder) takes the running app's simulator screen.
        from mcp_client import McpClient
        z.run("keys cmd-r")
        console_has(z, "HelloZ launched")
        client = McpClient([BINARY, "mcp"], env=dict(os.environ, ZCODE_CONFIG_DIR=CONFIG_DIR), cwd=samples)
        try:
            client.initialize()
            destinations = client.call("XcodeListRunDestinations")["structuredContent"]
            chosen = next(d for d in destinations["destinations"] if d["id"] == destinations["destination"])
            assert chosen["kind"] == "simulator" and chosen["booted"], chosen
            result = client.call("TakeScreenshot", {"target": "simulator"}, timeout=120)
            assert not result.get("isError"), result
            shot = result["structuredContent"]
            assert shot["simulator"]["udid"] == chosen["id"] and shot["height"] > shot["width"] > 300, shot
            image = next(b for b in result["content"] if b["type"] == "image")
            assert image["mimeType"] == "image/png" and base64.b64decode(image["data"])[:8] == b"\x89PNG\r\n\x1a\n"
        finally:
            client.close()
        z.run("keys cmd-.", "wait 1500")

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
    check("sourcekit-lsp ready through the build server (no false errors)", lsp_ready)
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
        check("MCP: a screenshot of the running app's simulator (simctl io screenshot)", mcp_simulator_screenshot)
        check("Debug on iOS Simulator (breakpoint stop, LLDB console)", debug_on_simulator)


FRESH_SWIFT = """func fresh() -> Int {
    var counter = Counter()
    counter.increment(by: 2)
    return counter.count
}

#if ZCODE_BSP
let flagged: Int = "not a number"
#endif
"""


def build_server_checks(samples):
    """zcode's build server (`zcode bsp`) on a copy of HelloZ: sourcekit-lsp
    finds it through zcode's own connection file and asks it for every
    file's arguments; a file added to the synchronized folder works before
    any build has seen it; a build setting edited in project.pbxproj (as
    Xcode would) reaches the open file; a membership exception takes a file
    out of its target; "compilation_database" goes back to the old path."""
    project = os.path.join(WORK, "BspHello")
    shutil.copytree(os.path.join(samples, "HelloZ"), project)
    app = os.path.join(project, "HelloZ")
    content, fresh = os.path.join(app, "ContentView.swift"), os.path.join(app, "Fresh.swift")
    pbxproj = os.path.join(project, "HelloZ.xcodeproj", "project.pbxproj")
    z = Zcode(project)

    def status():
        return json.loads(z.run("bsp status")[-1])

    def options(path):
        return json.loads(z.run(f"bsp options {path}")[-1])

    def wait_options(path, predicate, timeout=60):
        deadline = time.time() + timeout
        while time.time() < deadline:
            o = options(path)
            if predicate(o):
                return o
            time.sleep(0.5)
        raise TimeoutError(f"bsp options {path}: {o}")

    def fresh_issues(s):
        return [i for i in s["issues"] if "Fresh.swift" in i]

    def connects():
        z.run(f"open {content}")
        s = z.wait_for(lambda s: s["lsp"] == "Ready" and s["build_server"]["connections"]["open"] >= 1, timeout=120)
        b = s["build_server"]
        assert b["through_build_server"] and b["setting"] == "build_server", b
        assert b["targets"] >= 2 and b["active"], b
        # sourcekit-lsp asked for the open file's arguments.
        b = z.wait_for(lambda s: s["build_server"]["connections"]["requests"].get("textDocument/sourceKitOptions"), timeout=60)["build_server"]
        assert b["connections"]["last_options"][0] == os.path.realpath(content), b
        # Xcode's own arguments for indexing, once asked.
        z.wait_for(lambda s: not s["build_server"]["querying"] and s["build_server"]["swift_arguments"]["index-settings"] >= 1, timeout=120)
        o = options(content)
        assert o["targets"][0] == "HelloZ" and o["origin"] == "index-settings" and "-module-name" in o["arguments"], o
        # Nothing written into the project.
        assert not os.path.exists(os.path.join(project, "buildServer.json")) and not os.path.exists(os.path.join(project, ".bsp"))

    def new_file_without_a_build():
        changes = status()["connections"]["changes_sent"]
        with open(fresh, "w") as f:
            f.write(FRESH_SWIFT)
        # The synchronized folder changed: the next look has it in the target.
        o = wait_options(fresh, lambda o: o["targets"] == ["HelloZ"])
        assert os.path.realpath(fresh) in o["arguments"] and os.path.realpath(content) in o["arguments"], o
        z.wait_for(lambda s: s["build_server"]["connections"]["changes_sent"] > changes, timeout=30)
        line = line_of(fresh, "Counter()")
        for _ in range(10):
            z.run(f"open {fresh}:{line}", "wait 300", keys_right(column_of(fresh, "Counter()", 2)), "keys ctrl-cmd-j")
            try:
                s = z.wait_for(lambda s: s["active"]["path"].endswith("Counter.swift"), timeout=5, interval=0.5)
                break
            except TimeoutError:
                continue
        assert s["active"]["path"].endswith("Counter.swift"), s["active"]["path"]
        assert not fresh_issues(z.dump()), "ZCODE_BSP isn't set yet"

    def index_without_a_restart():
        # The index build's index, opened when sourcekit-lsp connected, fills
        # as the build writes it: references across files, no restart.
        z.wait_for(lambda s: not s["indexing"], timeout=300)
        counter = os.path.join(app, "Counter.swift")
        for _ in range(20):
            z.run(f"open {counter}:{line_of(counter, 'mutating func increment')}", "wait 300",
                  keys_right(column_of(counter, "increment", 2)), "keys ctrl-shift-cmd-f", "wait 1500")
            if json.loads(z.run("bench status")[-1])["find_files"] >= 2:
                break
        b = json.loads(z.run("bench status")[-1])
        assert b["find_files"] >= 2, (b["find_files"], b["find_results"])
        assert b["build_server"]["connections"]["connections"] == 1, b["build_server"]["connections"]

    def build_setting_reaches_the_open_file():
        z.run(f"open {fresh}")
        text = open(pbxproj).read()
        edited = text.replace('SWIFT_ACTIVE_COMPILATION_CONDITIONS = "DEBUG $(inherited)";',
                              'SWIFT_ACTIVE_COMPILATION_CONDITIONS = "DEBUG ZCODE_BSP $(inherited)";', 1)
        assert edited != text
        with open(pbxproj, "w") as f:
            f.write(edited)
        # Xcode is asked again; sourcekit-lsp gets the flag and type-checks
        # the block it guards, with nothing built.
        wait_options(fresh, lambda o: "-DZCODE_BSP" in (o["arguments"] or []), timeout=120)
        s = z.wait_for(lambda s: fresh_issues(s), timeout=60, interval=0.5)
        assert any(f"Fresh.swift:{line_of(fresh, 'flagged')}" in i for i in fresh_issues(s)), fresh_issues(s)

    def membership_exception():
        text = open(pbxproj).read()
        exception = ('/* Begin PBXFileSystemSynchronizedBuildFileExceptionSet section */\n'
                     '\t\tA1000000000000000000BB01 /* Exceptions for "HelloZ" folder in "HelloZ" target */ = {\n'
                     '\t\t\tisa = PBXFileSystemSynchronizedBuildFileExceptionSet;\n'
                     '\t\t\tmembershipExceptions = (\n\t\t\t\tFresh.swift,\n\t\t\t);\n'
                     '\t\t\ttarget = A10000000000000000000006 /* HelloZ */;\n\t\t};\n'
                     '/* End PBXFileSystemSynchronizedBuildFileExceptionSet section */\n\n')
        edited = text.replace("/* Begin PBXFileSystemSynchronizedRootGroup section */", exception + "/* Begin PBXFileSystemSynchronizedRootGroup section */", 1)
        edited = edited.replace("\t\tA10000000000000000000002 /* HelloZ */ = {\n\t\t\tisa = PBXFileSystemSynchronizedRootGroup;\n",
                                "\t\tA10000000000000000000002 /* HelloZ */ = {\n\t\t\tisa = PBXFileSystemSynchronizedRootGroup;\n"
                                "\t\t\texceptions = (\n\t\t\t\tA1000000000000000000BB01 /* Exceptions for \"HelloZ\" folder in \"HelloZ\" target */,\n\t\t\t);\n", 1)
        assert edited.count("A1000000000000000000BB01") == 2, "exception set added"
        with open(pbxproj, "w") as f:
            f.write(edited)
        wait_options(fresh, lambda o: o["targets"] == [])
        o = options(content)
        assert os.path.realpath(fresh) not in o["arguments"], o

    def compilation_database_setting():
        z.run('settings set sourcekit_build_settings "compilation_database"')
        s = z.wait_for(lambda s: not s["build_server"]["through_build_server"] and s["lsp"] == "Ready", timeout=120)
        z.run('settings set sourcekit_build_settings "build_server"')
        s = z.wait_for(lambda s: s["build_server"]["through_build_server"] and s["lsp"] == "Ready"
                       and s["build_server"]["connections"]["connections"] >= 2, timeout=120)

    try:
        check("Build server: sourcekit-lsp connects through zcode's connection file and asks it for arguments (Xcode's for indexing)", connects)
        check("Build server: a file added to a synchronized folder, never built, gets its target's arguments (⌃⌘J across files)", new_file_without_a_build)
        check("Build server: the index build's index reaches sourcekit-lsp without a restart (references across files)", index_without_a_restart)
        check("Build server: a build setting edited in project.pbxproj reaches the open file without a build", build_setting_reaches_the_open_file)
        check("Build server: a membership exception takes the file out of its target", membership_exception)
        check("Build server: \"compilation_database\" restarts sourcekit-lsp on the database, and back", compilation_database_setting)
    finally:
        z.quit()


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
    xctrace = fake_xctrace("devices")
    z = Zcode(project, {"ZCODE_DEVICECTL": os.path.join(ROOT, "scripts", "fake_devicectl.py"), "FAKE_DEVICECTL_DIR": state,
                        **xctrace.env})
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

        def listed():
            return len([c for c in calls() if c["args"][:2] == ["list", "devices"]])

        def hot_plug():
            # Listed as the destination menu opens (and on USB attach and
            # detach, which the fake can't make).
            set_devices(drop=("Test iPhone (USB)",))
            z.run("action zcode::ChooseDestination")
            z.wait_for(lambda s: not any("(USB)" in l for l in s["destination_menu"]), timeout=20, interval=0.5)
            z.run("keys escape")
            set_devices()
            z.run("action zcode::ChooseDestination")
            z.wait_for(lambda s: any("(USB)" in l for l in s["destination_menu"]), timeout=20, interval=0.5)
            z.run("keys escape")

        def not_polled():
            # devicectl costs ~60 ms of CPU a list: an idle window lists
            # once a minute, not every few seconds.
            before = listed()
            z.run("wait 11000")
            assert listed() - before <= 1, listed() - before

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
        check("Devices: an idle window doesn't list devices every few seconds", not_polled)
        check("Devices: Run on an unavailable device says why without building", unavailable_device)
        check("Devices: Run signs, installs, waits for unlock, launches with the scheme's arguments and environment; Stop terminates", run_and_stop)
        check("Devices: Run with breakpoints launches suspended and attaches lldb through CoreDevice", debug_attach)
        def profile_on_device():
            # Never a real device's: the fake devicectl and xctrace stand in.
            installs = len([c for c in calls() if c["args"][:3] == ["device", "install", "app"]])
            z.run(f"destination {udid}", "keys cmd-i")
            z.wait_for(lambda s: s["modal_open"] and isinstance(s["profile"]["templates"], list), timeout=30, interval=0.3)
            z.run("type Time Profiler", "keys enter")
            z.wait_for(lambda s: s["profile"]["recording"], timeout=600, interval=0.5)
            record = xctrace.records()[-1]
            assert record[record.index("--device") + 1] == udid, record
            assert record[record.index("--") + 1] == "dev.zcode.HelloZ" and "--target-stdout" not in record, record
            assert len([c for c in calls() if c["args"][:3] == ["device", "install", "app"]]) == installs + 1, "installed first"
            assert "-configuration Release" in open(z.dump()["last_log"]).read()
            z.run("keys cmd-.")
            s = z.wait_for(finished, timeout=60, interval=0.3)
            last = s["profile"]["last"]
            assert "success: true" in s["phase"] and last["opened"] and xctrace.opened()[-1] == last["trace"], (s["phase"], last)
            assert last["summary"]["process"] == "HelloZ", last["summary"]

        check("Devices: a signing error for Any iOS Device shows in the Issue navigator with where to fix it", signing_error)
        check("Devices: Profile (⌘I) builds Release, installs, and records the app on the device through xctrace", profile_on_device)
    finally:
        z.quit()


class fake_xctrace:
    """scripts/fake_xctrace.py's state for one zcode: its calls, and the
    traces zcode would have opened in Instruments."""

    def __init__(self, name):
        self.state = os.path.join(WORK, f"fake-xctrace-{name}")
        os.makedirs(self.state, exist_ok=True)
        self.opened_log = os.path.join(self.state, "opened.txt")
        self.env = {"ZCODE_XCTRACE": os.path.join(ROOT, "scripts", "fake_xctrace.py"), "FAKE_XCTRACE_DIR": self.state,
                    "ZCODE_OPEN_TRACE_LOG": self.opened_log}

    def records(self):
        try:
            with open(os.path.join(self.state, "calls.jsonl")) as f:
                return [c["args"] for c in map(json.loads, f) if c["args"][:1] == ["record"]]
        except FileNotFoundError:
            return []

    def opened(self):
        try:
            with open(self.opened_log) as f:
                return f.read().splitlines()
        except FileNotFoundError:
            return []

    def app_pid(self):
        with open(os.path.join(self.state, "app_pid")) as f:
            return int(f.read())


def profile_checks(samples, sim):
    """Product ▸ Profile through a fake xctrace (scripts/fake_xctrace.py):
    this Mac can't record without Developer Tools access, and a real
    recording would open Instruments on the user's screen. Traces go to the
    run's own config folder; their paths are logged instead of opened."""
    xctrace = fake_xctrace("mac")
    project = os.path.join(WORK, "Profile", "HelloClassic")
    shutil.copytree(os.path.join(samples, "HelloClassic"), project)
    scheme = os.path.join(project, "HelloClassic.xcodeproj", "xcshareddata", "xcschemes", "HelloClassic.xcscheme")
    traces = os.path.join(CONFIG_DIR, "traces")

    def chooser(z):
        return z.wait_for(lambda s: s["modal_open"] and isinstance(s["profile"]["templates"], list), timeout=30, interval=0.3)

    z = Zcode(project, xctrace.env)
    try:
        def time_profiler():
            z.wait_for(lambda s: s["scheme_options"], timeout=60)
            z.run("keys cmd-<", "wait 300", "edit-scheme argument -profiled", "edit-scheme variable GREETING=instruments",
                  "edit-scheme close")
            z.run("keys cmd-i")
            s = chooser(z)
            assert s["profile"]["templates"][:6] == ["Time Profiler", "Allocations", "Leaks", "Animation Hitches", "SwiftUI", "App Launch"], s["profile"]
            assert "Hitches and Leaks" in s["profile"]["templates"], "the user's templates too"
            z.run("type Time Profiler", "keys enter")
            # The Run action's arguments and environment; the app's output in the console.
            s = console_has(z, "-profiled GREETING=instruments", timeout=300)
            assert s["profile"]["recording"]["template"] == "Time Profiler" and not s["profile"]["recording"]["stopping"], s["profile"]
            assert "-configuration Release" in open(s["last_log"]).read(), "the Profile action's configuration"
            record = xctrace.records()[-1]
            assert record[record.index("--template") + 1] == "Time Profiler" and "--no-prompt" in record, record
            assert record[record.index("--target-stdout") + 1] == "-" and "GREETING=instruments" in record, record
            assert record[record.index("--") + 1].endswith("/Release/HelloClassic.app") and record[-1] == "-profiled", record
            assert record[record.index("--output") + 1].startswith(traces), record
            assert "--time-limit" not in record, record
            pid = xctrace.app_pid()
            z.run("keys cmd-.")
            s = z.wait_for(finished, timeout=60, interval=0.3)
            assert "success: true" in s["phase"], s["phase"]
            last = s["profile"]["last"]
            assert last["template"] == "Time Profiler" and last["opened"] and os.path.isdir(last["trace"]), last
            assert xctrace.opened() == [last["trace"]], "opened in Instruments"
            summary = last["summary"]
            assert summary["process"] == "HelloClassic" and summary["samples"] == 3, summary
            assert [t["function"] for t in summary["top"]] == ["Greeter.greeting.getter", "objc_msgSend"], summary
            tail = s["console_tail"]
            assert "Heaviest functions in HelloClassic (3 samples, 0.00 s):" in tail, tail
            assert "   66.7%   66.7%  Greeter.greeting.getter  (HelloClassic)" in tail, tail
            assert f"Saved {last['trace']}" in tail and "Opened the trace in Instruments" in tail, tail
            # The app the recording launched doesn't outlive it.
            process_gone(pid)

        def time_limit_without_building():
            reports = z.dump()["reports"]
            z.run("settings set profiling.time_limit 2", "keys ctrl-cmd-i")
            chooser(z)
            z.run("type Allocations", "keys enter")
            s = z.wait_for(lambda s: finished(s) and len(xctrace.opened()) == 2, timeout=60, interval=0.3)
            assert "success: true" in s["phase"] and s["reports"] == reports, "nothing built"
            record = xctrace.records()[-1]
            assert record[record.index("--template") + 1] == "Allocations", record
            assert record[record.index("--time-limit") + 1] == "2s", record
            assert s["profile"]["last"]["summary"] is None, "Allocations has no samples to summarize"
            process_gone(xctrace.app_pid())
            z.run("settings set profiling.time_limit 0")

        def scheme_instrument():
            original = open(scheme).read()
            with open(scheme, "w") as f:
                f.write(original.replace('savedToolIdentifier = ""', 'savedToolIdentifier = "Leaks"'))
            try:
                records = len(xctrace.records())
                z.run("keys cmd-i", "wait 300")
                assert not z.dump()["modal_open"], "the scheme's instrument: no chooser"
                z.wait_for(lambda s: s["profile"]["recording"] and len(xctrace.records()) > records, timeout=300, interval=0.5)
                record = xctrace.records()[-1]
                assert record[record.index("--template") + 1] == "Leaks", record
                z.run("keys cmd-.")
                z.wait_for(finished, timeout=60, interval=0.3)
                # Profile… asks all the same.
                z.run("keys cmd-alt-i")
                chooser(z)
                z.run("keys escape", "wait 200")
                assert not z.dump()["modal_open"]
            finally:
                with open(scheme, "w") as f:
                    f.write(original)

        def build_for_profiling():
            records = len(xctrace.records())
            z.run("keys cmd-shift-i")
            s = z.wait_for(finished, timeout=300, interval=0.5)
            assert s["phase"].startswith("Finished { action: Build, success: true"), s["phase"]
            assert "-configuration Release" in open(s["last_log"]).read()
            assert len(xctrace.records()) == records, "nothing recorded"

        check("Profile (⌘I): template chooser, Release build, xctrace with the scheme's options; Stop saves, opens and summarizes", time_profiler)
        check("Profile Without Building (⌃⌘I) with profiling.time_limit: no build, the recording ends itself", time_limit_without_building)
        check("Profile: the scheme's instrument skips the chooser; Profile… (⌥⌘I) asks anyway", scheme_instrument)
        check("Build For ▸ Profiling (⇧⌘I) builds Release without recording", build_for_profiling)
    finally:
        z.quit()

    if not sim:
        return
    simulator = fake_xctrace("simulator")
    project = os.path.join(WORK, "Profile", "HelloZ")
    shutil.copytree(os.path.join(samples, "HelloZ"), project)
    z = Zcode(project, simulator.env)
    try:
        def profile_on_simulator():
            udid = available_simulator_udid()
            z.wait_for(lambda s: s["destinations"] > 0, timeout=60)
            z.run(f"destination {udid}", "keys cmd-i")
            chooser(z)
            z.run("type SwiftUI", "keys enter")
            s = z.wait_for(lambda s: s["profile"]["recording"], timeout=600, interval=0.5)
            assert any(l.startswith("Installing HelloZ.app") for l in s["console_tail"]), s["console_tail"]
            record = simulator.records()[-1]
            assert record[record.index("--device") + 1] == udid, record
            assert record[record.index("--") + 1].endswith("Release-iphonesimulator/HelloZ.app"), record
            z.run("keys cmd-.")
            s = z.wait_for(finished, timeout=60, interval=0.3)
            assert "success: true" in s["phase"] and simulator.opened() == [s["profile"]["last"]["trace"]], s["profile"]

        check("Profile on a simulator: installs the Release build and records it there", profile_on_simulator)
    finally:
        z.quit()


class fake_distribute:
    """scripts/fake_distribute.py's state for one zcode: the exports,
    uploads, submissions and staples it pretended."""

    def __init__(self, name):
        self.state = os.path.join(WORK, f"fake-distribute-{name}")
        os.makedirs(self.state, exist_ok=True)
        fake = os.path.join(ROOT, "scripts", "fake_distribute.py")
        self.env = {"ZCODE_EXPORT_ARCHIVE": fake, "ZCODE_NOTARYTOOL": fake, "ZCODE_STAPLER": fake, "FAKE_DISTRIBUTE_DIR": self.state}

    def calls(self, tool):
        try:
            with open(os.path.join(self.state, "calls.jsonl")) as f:
                return [c for c in map(json.loads, f) if c["tool"] == tool]
        except FileNotFoundError:
            return []

    def stapled(self):
        try:
            with open(os.path.join(self.state, "stapled.txt")) as f:
                return f.read().splitlines()
        except FileNotFoundError:
            return []


def xcode_archive(folder, name, team, created):
    """An iOS app's archive as Xcode leaves it in its Archives folder: an
    XML Info.plist, and a binary one in the app."""
    path = os.path.join(folder, f"{name}.xcarchive")
    app = os.path.join(path, "Products", "Applications", f"{name}.app")
    os.makedirs(app)
    with open(os.path.join(app, "Info.plist"), "wb") as f:
        plistlib.dump({"CFBundleIdentifier": f"dev.zcode.{name}", "DTPlatformName": "iphoneos"}, f, fmt=plistlib.FMT_BINARY)
    with open(os.path.join(path, "Info.plist"), "wb") as f:
        plistlib.dump({
            "ApplicationProperties": {
                "ApplicationPath": f"Applications/{name}.app", "Architectures": ["arm64"], "CFBundleIdentifier": f"dev.zcode.{name}",
                "CFBundleShortVersionString": "2.0", "CFBundleVersion": "12", "SigningIdentity": "Apple Development: E2E (TEAM)", "Team": team,
            },
            "ArchiveVersion": 2, "CreationDate": created, "Name": name, "SchemeName": name, "Comment": "From Xcode",
        }, f)
    return path


def archive_checks(samples):
    """Product ▸ Archive and the Organizer on a copy of HelloClassic, into the
    run's own Archives folder (ZCODE_CONFIG_DIR's, never
    ~/Library/Developer/Xcode/Archives): a real `xcodebuild archive` with
    Sign to Run Locally, the Organizer on it and on an archive Xcode left,
    a real Copy App export, a real Developer ID export refused for want of a
    team; HelloZ's signing error. Then App Store Connect and TestFlight
    uploads, notarization (accepted, rejected, no credentials) and a failed
    upload through scripts/fake_distribute.py, which pretends: nothing is
    uploaded or sent to Apple. ArchiveProject over MCP; Delete Archive."""
    import datetime
    project = os.path.join(WORK, "Archive", "HelloClassic")
    shutil.copytree(os.path.join(samples, "HelloClassic"), project)
    archives_dir = os.path.join(CONFIG_DIR, "archives")
    exports = os.path.join(WORK, "Archive", "exports")
    archived = {}

    def organizer(s):
        return s["archives"]["organizer"] or {}

    def shown(s):
        return any(p["active"] == "Organizer" for p in s["panes"]["list"])

    def distribution(z, *commands, timeout=300):
        z.run("organizer distribute", "wait 300")
        assert z.dump()["archives"]["sheet"], "the sheet is open"
        before = z.dump()["reports"]
        z.run(*[f"distribute {c}" for c in commands], "distribute confirm")
        s = z.wait_for(lambda s: finished(s) and s["reports"] > before, timeout=timeout, interval=0.5)
        assert not s["modal_open"], "the sheet closed"
        return s

    z = Zcode(project)
    try:
        def archive_a_mac_app():
            # Once the scheme's platforms are known (macOS only).
            z.wait_for(lambda s: s["scheme_options"] and (s["destination"] or "").startswith("My Mac"), timeout=60)
            z.run("action zcode::Archive")
            s = z.wait_for(finished, timeout=600, interval=0.5)
            assert s["phase"].startswith("Finished { action: Archive, success: true"), (s["phase"], s["issues"])
            assert s["destination"].startswith("My Mac"), "the run destination stays"
            last = s["archives"]["last_archive"]["archive"]
            path = last["path"]
            archived["path"] = path
            day = datetime.date.today().isoformat()
            assert path.startswith(os.path.join(archives_dir, day) + "/") and os.path.basename(path).startswith("HelloClassic "), path
            assert os.path.isdir(os.path.join(path, "Products", "Applications", "HelloClassic.app")), path
            assert last["app"]["platform"] == "macOS" and last["app"]["bundle_id"] == "dev.zcode.HelloClassic" and last["app"]["team"] == "", last
            log = open(s["last_log"]).read()
            assert " archive " in log and "-configuration Release" in log and "generic/platform=macOS" in log, log[:600]
            assert "-allowProvisioningUpdates" in log and f'-archivePath "{path}"' in log, log[:600]
            assert s["last_report"] == {"action": "Archive", "scheme": "HelloClassic", "destination": "Any Mac (Apple Silicon, Intel)", "success": True}, s["last_report"]
            # The Organizer opens on it.
            s = z.wait_for(lambda s: organizer(s).get("loaded") and organizer(s).get("selected") == path, timeout=30, interval=0.3)
            assert shown(s) and organizer(s)["group"] == "HelloClassic · macOS", organizer(s)
            assert not os.path.exists(os.path.expanduser("~/Library/Developer/Xcode/Archives/" + day + "/" + os.path.basename(path))), "not Xcode's folder"

        def organizer_lists_xcodes_archives():
            folder = os.path.join(archives_dir, "2026-01-02")
            xcode_archive(folder, "HelloPhone", "ABCDE12345", datetime.datetime(2026, 1, 2, 9, 30))
            z.run("organizer refresh", "wait 500")
            o = organizer(z.dump())
            assert [g["name"] for g in o["groups"]] == ["HelloClassic · macOS", "HelloPhone · iOS"], o["groups"]
            assert o["selected"] == archived["path"], "the selection stays"
            z.run("organizer select HelloPhone")
            o = organizer(z.dump())
            phone = o["listed"][0]
            assert o["group"] == "HelloPhone · iOS" and phone["app"]["version"] == "2.0" and phone["app"]["team"] == "ABCDE12345", phone
            assert phone["comment"] == "From Xcode" and phone["kind"] == "iOS App", phone
            z.run("organizer distribute", "wait 300")
            sheet = z.dump()["archives"]["sheet"]
            assert sheet["methods"] == ["app-store-connect", "testflight-internal", "release-testing", "enterprise", "debugging"], sheet
            assert sheet["method"] == "app-store-connect" and sheet["upload"] and sheet["team"] == "ABCDE12345", sheet
            z.run("distribute method release-testing")
            options = plistlib.loads(z.dump()["archives"]["sheet"]["export_options"].encode())
            assert options == {"destination": "export", "method": "release-testing", "stripSwiftSymbols": True, "teamID": "ABCDE12345", "thinning": "<none>"}, options
            z.run("distribute cancel", "wait 200")
            assert not z.dump()["modal_open"]
            z.run(f"organizer select {archived['path']}")

        def copy_app():
            folder = os.path.join(exports, "Copy")
            z.run("organizer distribute", "wait 300")
            sheet = z.dump()["archives"]["sheet"]
            assert sheet["methods"] == ["app-store-connect", "testflight-internal", "developer-id", "copy-app", "debugging"], sheet
            assert sheet["folder"].startswith(os.path.join(CONFIG_DIR, "exports", "HelloClassic ")), sheet["folder"]
            z.run("distribute cancel", "wait 200")
            s = distribution(z, "method copy-app", f"folder {folder}")
            assert s["phase"].startswith("Finished { action: Distribute, success: true"), (s["phase"], s["issues"])
            d = s["archives"]["last_distribution"]
            app = os.path.join(folder, "HelloClassic.app")
            assert d["method"] == "copy-app" and not d["uploaded"] and d["products"] == [app] and d["error"] is None, d
            subprocess.run(["codesign", "--verify", app], check=True)
            log = open(s["last_log"]).read()
            assert "-exportArchive" in log and "** EXPORT SUCCEEDED **" in log, log[-600:]
            assert s["last_report"] == {"action": "Distribute", "scheme": "HelloClassic", "destination": "Copy App", "success": True}, s["last_report"]

        def developer_id_needs_a_team():
            s = distribution(z, "method developer-id", "notarize off", f"folder {os.path.join(exports, 'DeveloperID')}")
            assert "success: false" in s["phase"], s["phase"]
            errors = [i for i in s["issues"] if i.startswith("Error")]
            assert any("No Team Found in Archive. Choose the team and signing in the target’s Signing & Capabilities tab" in i for i in errors), errors
            assert s["archives"]["last_distribution"]["error"], s["archives"]["last_distribution"]

        def organizer_shortcut():
            z.run("open " + os.path.join(project, "HelloClassic", "Model", "Greeter.swift"))
            z.wait_for(lambda s: not shown(s), timeout=10, interval=0.3)
            z.run("keys cmd-alt-shift-o")
            z.wait_for(shown, timeout=10, interval=0.3)

        check("Archive: a Release archive of the Mac app for any Mac, in the Archives folder, revealed in the Organizer", archive_a_mac_app)
        check("Organizer: archives Xcode left too, grouped by app; Distribute App's methods and export options per platform", organizer_lists_xcodes_archives)
        check("Distribute App: Copy App exports the app with xcodebuild -exportArchive; a Distribute report", copy_app)
        check("Distribute App: Developer ID without a team fails, pointing at Signing & Capabilities", developer_id_needs_a_team)
        check("Window ▸ Organizer (⌥⇧⌘O) shows the Organizer", organizer_shortcut)
    finally:
        z.quit()

    signing = os.path.join(WORK, "Archive", "HelloZ")
    shutil.copytree(os.path.join(samples, "HelloZ"), signing)
    z = Zcode(signing)
    try:
        def signing_error():
            z.wait_for(lambda s: s["scheme_options"] and s["destinations"] > 0, timeout=60)
            z.run("action zcode::Archive")
            s = z.wait_for(finished, timeout=600, interval=0.5)
            assert s["phase"].startswith("Finished { action: Archive, success: false"), s["phase"]
            assert any("requires a development team. Choose the team in the target’s Signing & Capabilities tab" in i for i in s["issues"]), s["issues"]
            assert "generic/platform=iOS" in open(s["last_log"]).read()
            last = s["archives"]["last_archive"]
            assert last["archive"] is None and "development team" in last["error"], last
            assert not any(d.startswith("HelloZ ") for _, dirs, _ in os.walk(archives_dir) for d in dirs), "no archive"

        check("Archive: an iOS app without a team fails with where to choose one (Signing & Capabilities)", signing_error)
    finally:
        z.quit()

    fake = fake_distribute("organizer")
    z = Zcode(project, fake.env)
    try:
        def app_store_upload():
            z.run("organizer open")
            z.wait_for(lambda s: organizer(s).get("loaded"), timeout=30, interval=0.3)
            z.run(f"organizer select {archived['path']}")
            s = distribution(z, "symbols off")
            assert "success: true" in s["phase"], (s["phase"], s["issues"])
            call = fake.calls("xcodebuild")[-1]
            assert call["options"] == {"destination": "upload", "manageAppVersionAndBuildNumber": True, "method": "app-store-connect", "uploadSymbols": False}, call
            assert "-allowProvisioningUpdates" in call["args"] and call["args"][call["args"].index("-archivePath") + 1] == archived["path"], call
            d = s["archives"]["last_distribution"]
            assert d["uploaded"] and d["products"] == [] and d["error"] is None, d
            assert d["export_path"].startswith(os.path.expanduser("~/Library/Caches/zcode/projects/")) and "/Uploads/HelloClassic " in d["export_path"], d
            assert s["last_report"]["destination"] == "App Store Connect" and s["last_report"]["success"], s["last_report"]

        def testflight_export():
            folder = os.path.join(exports, "TestFlight")
            s = distribution(z, "method testflight-internal", "upload off", f"folder {folder}")
            assert "success: true" in s["phase"], (s["phase"], s["issues"])
            options = fake.calls("xcodebuild")[-1]["options"]
            assert options["destination"] == "export" and options["testFlightInternalTestingOnly"] is True, options
            assert fake.calls("xcodebuild")[-1]["args"][fake.calls("xcodebuild")[-1]["args"].index("-exportPath") + 1] == folder
            assert not s["archives"]["last_distribution"]["uploaded"]

        def notarized():
            folder = os.path.join(exports, "Notarized")
            s = distribution(z, "method developer-id", "notarize on", "profile zcode-e2e", f"folder {folder}")
            assert "success: true" in s["phase"], (s["phase"], s["issues"])
            app = os.path.join(folder, "HelloClassic.app")
            submit = fake.calls("notarytool")[-1]
            assert submit["args"][:1] == ["submit"] and submit["args"][submit["args"].index("--keychain-profile") + 1] == "zcode-e2e", submit
            assert "--wait" in submit["args"] and "HelloClassic.app/Contents/MacOS/HelloClassic" in submit["zip"], submit
            assert not os.path.exists(submit["args"][1]), "the zip is removed"
            assert fake.stapled() == [app], fake.stapled()
            d = s["archives"]["last_distribution"]
            assert d["stapled"] and d["notarization"]["status"] == "Accepted" and d["products"] == [app], d
            settings = json.loads(re_strip_comments(open(os.path.join(CONFIG_DIR, "settings.json")).read()))
            assert settings["distribution"]["notary_keychain_profile"] == "zcode-e2e", settings
            # The sheet offers it next time.
            z.run("organizer distribute", "wait 300")
            assert z.dump()["archives"]["sheet"]["profile"] == "zcode-e2e"
            z.run("distribute cancel", "wait 200")

        def notarization_refused():
            s = distribution(z, "method developer-id", "profile rejected", f"folder {os.path.join(exports, 'Rejected')}")
            assert "success: false" in s["phase"], s["phase"]
            assert any("The binary is not signed with a valid Developer ID certificate." in i for i in s["issues"]), s["issues"]
            d = s["archives"]["last_distribution"]
            assert d["notarization"]["status"] == "Invalid" and not d["stapled"] and "didn’t accept HelloClassic.app" in d["error"], d
            assert fake.calls("notarytool")[-1]["args"][:2] == ["log", "2efe2717-52ef-43a5-96dc-0797e4ca1041"]
            s = distribution(z, "method developer-id", "profile missing", f"folder {os.path.join(exports, 'Missing')}")
            assert "xcrun notarytool store-credentials missing" in s["archives"]["last_distribution"]["error"], s["archives"]["last_distribution"]

        def failed_upload():
            open(os.path.join(fake.state, "fail_upload"), "w").close()
            try:
                s = distribution(z, "method app-store-connect")
            finally:
                os.remove(os.path.join(fake.state, "fail_upload"))
            assert "success: false" in s["phase"] and s["archives"]["last_distribution"]["error"] == "The upload failed (see the build log)"
            assert any("The bundle version must be higher" in i for i in s["issues"]), s["issues"]

        def archive_project_over_mcp():
            from mcp_client import McpClient
            client = McpClient([BINARY, "mcp"], env=dict(os.environ, ZCODE_CONFIG_DIR=CONFIG_DIR), cwd=project)
            try:
                client.initialize()
                names = [t["name"] for t in client.request("tools/list")["tools"]]
                assert "ArchiveProject" in names, names
                r = client.call("ArchiveProject", timeout=600)["structuredContent"]
                assert r["success"] and r["status"] == "succeeded", r
                path = r["archive"]["path"]
                assert path.startswith(archives_dir) and path != archived["path"] and os.path.isdir(path), r
                assert r["archive"]["app"]["bundle_id"] == "dev.zcode.HelloClassic" and r["errors"] == [], r
            finally:
                client.close()
            calls = len(fake.calls("xcodebuild"))
            z.wait_for(lambda s: organizer(s).get("selected") == path, timeout=30, interval=0.3)
            assert len(fake.calls("xcodebuild")) == calls, "archiving never exports"
            archived["second"] = path

        def delete_archive():
            path = archived["second"]
            z.run(f"organizer select {path}", "organizer delete")
            o = organizer(z.dump())
            assert not os.path.exists(path) and o["selected"] == archived["path"], o
            trashed = os.path.expanduser(os.path.join("~/.Trash", os.path.basename(path)))
            assert os.path.isdir(trashed), "in the Trash"
            shutil.rmtree(trashed)

        check("Distribute App: App Store Connect uploads (destination upload, its options) — faked, nothing sent", app_store_upload)
        check("Distribute App: TestFlight Internal Only exported instead of uploaded", testflight_export)
        check("Distribute App: Developer ID notarized with a keychain profile and stapled (faked notarytool); the profile kept", notarized)
        check("Distribute App: a rejected notarization shows Apple's log; missing credentials say how to store them", notarization_refused)
        check("Distribute App: a refused upload fails with App Store Connect's message", failed_upload)
        check("MCP: ArchiveProject archives (never exports or uploads) and the Organizer shows it", archive_project_over_mcp)
        check("Organizer: Delete Archive moves it to the Trash", delete_archive)
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


def png_bytes(w, h, rgba):
    """A solid-color PNG."""
    import struct
    import zlib
    raw = b"".join(b"\x00" + bytes(rgba) * w for _ in range(h))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def tree_snapshot(root):
    """Every file under `root` with its bytes."""
    out = {}
    for folder, _, files in os.walk(root):
        for name in files:
            path = os.path.join(folder, name)
            out[os.path.relpath(path, root)] = open(path, "rb").read()
    return out


def asset_catalog_checks(samples):
    """The asset catalog editor on a copy of HelloClassic (group-based: the
    catalog is one file reference) whose Assets.xcassets gets the fixture
    catalog's sets (tests/fixtures/xcassets, Contents.json as Xcode writes
    them), and a copy of HelloZ (synchronized) given that catalog. Files
    are dropped through the window's own file-drop events (as Finder's
    drags arrive); fields are typed into. Every edit's Contents.json is
    checked byte for byte, then undone to the original bytes, and actool
    compiles what zcode wrote."""
    fixture = os.path.join(ROOT, "tests", "fixtures", "xcassets", "Assets.xcassets")
    project = os.path.join(WORK, "Assets", "HelloClassic")
    shutil.copytree(os.path.join(samples, "HelloClassic"), project)
    catalog = os.path.join(project, "HelloClassic", "Assets.xcassets")
    for name in os.listdir(fixture):
        if name != "Contents.json":
            shutil.copytree(os.path.join(fixture, name), os.path.join(catalog, name))
    pbxproj = os.path.join(project, "HelloClassic.xcodeproj", "project.pbxproj")
    pbxproj_before = open(pbxproj, "rb").read()
    original = tree_snapshot(catalog)
    drops = os.path.join(WORK, "Assets", "drops")
    os.makedirs(drops)
    orange = os.path.join(drops, "orange.png")
    open(orange, "wb").write(png_bytes(48, 48, (255, 149, 0, 255)))
    hero2x = os.path.join(drops, "hero@2x.png")
    open(hero2x, "wb").write(png_bytes(64, 64, (0, 122, 255, 255)))
    hero3x = os.path.join(drops, "hero@3x.png")
    open(hero3x, "wb").write(png_bytes(96, 96, (0, 122, 255, 255)))
    contents = lambda *rel: open(os.path.join(catalog, *rel, "Contents.json")).read()

    def assets(z):
        return z.dump()["asset_catalog"] or {}

    def wait_assets(z, predicate, timeout=30):
        deadline = time.time() + timeout
        while True:
            a = assets(z)
            if a and not a["busy"] and predicate(a):
                return a
            if time.time() > deadline:
                a = {k: v for k, v in a.items() if k not in ("outline", "places")}
                raise TimeoutError("asset catalog: " + json.dumps(a)[:1500])
            time.sleep(0.2)

    def edited(z, count):
        return wait_assets(z, lambda a: len(a["undo"]) == count)

    z = Zcode(project)
    try:
        def opens_as_a_tab():
            z.run(f"open {catalog}")
            a = wait_assets(z, lambda a: a["loaded"])
            s = z.dump()
            assert any(p["active"] == "Assets.xcassets" for p in s["panes"]["list"]), s["panes"]
            assert [o["name"] for o in a["outline"]] == ["AccentColor", "AppIcon", "Brand", "Icons", "Logo", "Seed"], a["outline"]
            assert a["sets"] == 7 and a["type"] == "Asset Catalog", a
            # A set opens the catalog on it, in the same tab.
            z.run(f"open {os.path.join(catalog, 'Logo.imageset')}")
            a = wait_assets(z, lambda a: a["selected_name"] == "Logo")
            assert sum(p["tabs"].count("Assets.xcassets") for p in z.dump()["panes"]["list"]) == 1
            assert [(sl["row"], sl["label"], sl["filename"]) for sl in a["slots"][:3]] == [
                ("Universal · Any Appearance", "1x", "logo.png"),
                ("Universal · Any Appearance", "2x", "logo@2x.png"),
                ("Universal · Any Appearance", "3x", None)], a["slots"]
            assert a["unassigned"] == ["stray.png"] and a["properties"] == [["template-rendering-intent", "original"]], a
            a = wait_assets(z, lambda a: [sl["thumbnail"] for sl in a["slots"][:2]] == ["ready", "ready"])

        check("Asset catalog: an .xcassets opens as a tab (a set opens it on that set); the outline, rows and wells as Xcode lays them out, PNG thumbnails decoded off the main thread", opens_as_a_tab)

        def vector_and_other_sets():
            z.run(f"open {catalog}", "assets select Icons/Badge")
            a = wait_assets(z, lambda a: a["slots"] and a["slots"][0]["thumbnail"] == "ready")
            assert a["type"] == "Image Set" and a["slots"][0]["label"] == "Single Scale", a["slots"]
            z.run("assets slot 0", "wait 200")
            assert assets(z)["properties"] == [["preserves-vector-representation", "true"]]
            z.run("assets select Icons/Star")
            a = wait_assets(z, lambda a: a["slots"] and a["slots"][0]["thumbnail"] == "ready")
            assert a["type"] == "Symbol Image Set", a
            z.run("assets select AppIcon")
            a = wait_assets(z, lambda a: a["slots"] and a["slots"][0]["thumbnail"] == "ready")
            assert [sl["row"] + " " + sl["label"] for sl in a["slots"]] == [
                "iOS 1024pt Any Appearance", "iOS 1024pt Dark", "iOS 1024pt Tinted", "Mac 16pt 1x", "Mac 16pt 2x"], a["slots"]
            z.run("assets select Seed")
            a = wait_assets(z, lambda a: a["type"] == "Data Set")
            assert a["slots"][0]["filename"] == "seed.json", a["slots"]

        check("Asset catalog: a PDF drawn by Core Graphics, an SVG symbol by resvg, app icon wells by platform and appearance, a data set", vector_and_other_sets)

        def color_components():
            z.run(f"open {catalog}", "assets select Brand")
            a = wait_assets(z, lambda a: a["type"] == "Color Set")
            assert [(sl["label"], sl["color"]) for sl in a["slots"]] == [("Any Appearance", "#2E4A73"), ("Dark", "#7A98BF")], a["slots"]
            # Click the Dark well, then type over Red in its 8-bit hex field.
            x, y = a["places"]["well:1"]
            z.run(f"click {x} {y}", "wait 300")
            a = assets(z)
            assert a["slot"] == 1 and a["components"] == [["red", "0x7A"], ["green", "0x98"], ["blue", "0xBF"], ["alpha", "1.000"]], a["components"]
            z.run("assets field red", "keys cmd-a", "type 0xFF", "keys enter")
            a = edited(z, 1)
            assert a["slots"][1]["color"] == "#FF98BF" and a["undo"] == ["Change Color"], a
            expected = original["Brand.colorset/Contents.json"].decode().replace('"red" : "0x7A"', '"red" : "0xFF"')
            assert contents("Brand.colorset") == expected, contents("Brand.colorset")
            # Alpha is floating point whatever the input method.
            z.run("assets color Dark alpha 0.5")
            edited(z, 2)
            assert '"alpha" : "0.500",\n          "blue" : "0xBF"' in contents("Brand.colorset"), contents("Brand.colorset")
            z.run("assets method Dark float")
            a = edited(z, 3)
            assert '"red" : "1.000"' in contents("Brand.colorset") and a["slots"][1]["color"] == "#FF98BF80", a["slots"]
            # ⌘Z three times: the file Xcode wrote, byte for byte.
            x, y = a["places"]["well:0"]
            z.run(f"click {x} {y}", "keys cmd-z")
            edited(z, 2)
            z.run("keys cmd-z")
            edited(z, 1)
            z.run("keys cmd-z")
            a = edited(z, 0)
            assert a["redo"] == ["Change Input Method", "Change Color", "Change Color"], a["redo"]
            assert open(os.path.join(catalog, "Brand.colorset", "Contents.json"), "rb").read() == original["Brand.colorset/Contents.json"], contents("Brand.colorset")
            z.run("keys cmd-shift-z")
            edited(z, 1)
            assert '"red" : "0xFF"' in contents("Brand.colorset"), contents("Brand.colorset")
            z.run("keys cmd-z")
            edited(z, 0)

        check("Asset catalog: color swatches from Contents.json; a component typed into its field written in its input method as Xcode does; ⌘Z back to Xcode's bytes, ⇧⌘Z", color_components)

        def drops_into_wells():
            z.run(f"open {catalog}", "assets select Logo")
            a = wait_assets(z, lambda a: a["selected_name"] == "Logo" and "well:2" in a["places"])
            x, y = a["places"]["well:2"]
            z.run(f"file-drop {x} {y} {orange}")
            a = edited(z, 1)
            assert a["slots"][2]["filename"] == "orange.png" and a["undo"] == ["Drop Image"], a["slots"]
            assert open(os.path.join(catalog, "Logo.imageset", "orange.png"), "rb").read() == open(orange, "rb").read()
            assert '{\n      "filename" : "orange.png",\n      "idiom" : "universal",\n      "scale" : "3x"\n    },' in contents("Logo.imageset"), contents("Logo.imageset")
            a = wait_assets(z, lambda a: a["slots"][2]["thumbnail"] == "ready")
            # Dropped again on the 2x well: logo@2x.png goes, orange.png is
            # kept by the 3x well, so the copy is "orange 1.png".
            x, y = a["places"]["well:1"]
            z.run(f"file-drop {x} {y} {orange}")
            a = edited(z, 2)
            assert a["slots"][1]["filename"] == "orange 1.png", a["slots"]
            assert not os.path.exists(os.path.join(catalog, "Logo.imageset", "logo@2x.png"))
            # Delete on a well clears it.
            z.run("assets slot 3x", "keys backspace")
            a = edited(z, 3)
            assert a["slots"][2]["filename"] is None and not os.path.exists(os.path.join(catalog, "Logo.imageset", "orange.png")), a["slots"]
            for count in (2, 1, 0):
                z.run("keys cmd-z")
                edited(z, count)
            assert tree_snapshot(catalog) == original, "undone byte for byte"
            # Not an image: refused, nothing written.
            try:
                z.run(f"assets drop 1x {os.path.join(catalog, 'Seed.dataset', 'seed.json')}")
                raise AssertionError("a JSON file dropped into an image well")
            except RuntimeError as e:
                assert "isn’t an image" in str(e), e
            assert not assets(z)["undo"]

        check("Asset catalog: a file dropped from Finder into a well is copied into the set and named in Contents.json (the old file goes unless another slot keeps it); Delete clears a well; ⌘Z undoes it all", drops_into_wells)

        def sets_added_renamed_removed():
            files_before = z.dump()["files"]
            z.run(f"open {catalog}", "assets select /", "assets new image")
            a = edited(z, 1)
            assert a["selected_name"] == "Image" and [sl["label"] for sl in a["slots"]] == ["1x", "2x", "3x"], a
            assert contents("Image.imageset") == ('{\n  "images" : [\n    {\n      "idiom" : "universal",\n      "scale" : "1x"\n    },\n'
                                                  '    {\n      "idiom" : "universal",\n      "scale" : "2x"\n    },\n'
                                                  '    {\n      "idiom" : "universal",\n      "scale" : "3x"\n    }\n  ],\n'
                                                  '  "info" : {\n    "author" : "xcode",\n    "version" : 1\n  }\n}\n'), contents("Image.imageset")
            z.wait_for(lambda s: s["files"] == files_before + 1, timeout=15, interval=0.3)
            # Renamed by typing its name.
            z.run("assets field name", "keys cmd-a", "type Hero", "keys enter")
            a = edited(z, 2)
            assert a["selected_name"] == "Hero" and os.path.isdir(os.path.join(catalog, "Hero.imageset")), a
            z.run("assets new color")
            a = edited(z, 3)
            assert a["selected_name"] == "Color" and contents("Color.colorset") == (
                '{\n  "colors" : [\n    {\n      "idiom" : "universal"\n    }\n  ],\n  "info" : {\n    "author" : "xcode",\n    "version" : 1\n  }\n}\n')
            try:
                z.run("assets rename Brand")
                raise AssertionError("renamed over Brand")
            except RuntimeError as e:
                assert "already exists" in str(e), e
            assert len(assets(z)["undo"]) == 3
            # An unset color gets its first component.
            z.run("assets slot 0", "assets color red 0.5")
            a = edited(z, 4)
            assert a["slots"][0]["color"] == "#80FFFF", a["slots"]
            # Files dropped on the outline: an image set named for them, by scale.
            x, y = a["places"]["row:Seed"]
            z.run(f"file-drop {x} {y} {hero2x};{hero3x}")
            a = edited(z, 5)
            assert a["selected_name"] == "hero 1" and [(sl["label"], sl["filename"]) for sl in a["slots"]] == [
                ("1x", None), ("2x", "hero@2x.png"), ("3x", "hero@3x.png")], a["slots"]
            # Remove Selected Items with Delete.
            x, y = a["places"]["row:Hero"]
            z.run(f"click {x} {y}", "keys backspace")
            a = edited(z, 6)
            assert "Hero" not in [o["name"] for o in a["outline"]] and not os.path.exists(os.path.join(catalog, "Hero.imageset")), a["outline"]
            assert open(pbxproj, "rb").read() == pbxproj_before, "project.pbxproj untouched (the catalog is one file reference)"
            for count in range(5, -1, -1):
                z.run("assets undo")
                edited(z, count)
            assert tree_snapshot(catalog) == original, "every edit undone, byte for byte"
            z.wait_for(lambda s: s["files"] == files_before, timeout=15, interval=0.3)

        check("Asset catalog: New Image Set and New Color Set written as Xcode writes them, renamed by typing, files dropped on the outline become a set by scale, Delete removes a set; project.pbxproj untouched; ⌘Z all the way back", sets_added_renamed_removed)

        def mcp_open_file():
            from mcp_client import McpClient
            c = McpClient([BINARY, "mcp"], cwd=project)
            try:
                c.initialize()
                r = c.call("OpenFile", {"path": "HelloClassic/Assets.xcassets/Icons/Star.symbolset"})
                assert not r.get("isError"), r
                a = wait_assets(z, lambda a: a["selected_name"] == "Star")
                assert a["type"] == "Symbol Image Set", a
            finally:
                c.close()

        check("Asset catalog: MCP's OpenFile on a set opens the catalog on it", mcp_open_file)

        def reopened_after_relaunch():
            z.run(f"open {os.path.join(catalog, 'Brand.colorset')}", "wait 300")
            z.quit()
            z2 = Zcode(project)
            try:
                a = z2.wait_for(lambda s: s["asset_catalog"] and s["asset_catalog"]["loaded"], timeout=30, interval=0.3)["asset_catalog"]
                assert a["root"] == os.path.realpath(catalog), a["root"]
            finally:
                z2.quit()

        check("Asset catalog: the tab comes back after a relaunch", reopened_after_relaunch)
    finally:
        z.quit()

    def synchronized_project_and_actool():
        hello = os.path.join(WORK, "Assets", "HelloZ")
        shutil.copytree(os.path.join(samples, "HelloZ"), hello)
        assets_dir = os.path.join(hello, "HelloZ", "Assets.xcassets")
        shutil.copytree(fixture, assets_dir)
        # Not what actool takes: a stray file, a toy SVG that isn't an SF
        # Symbols template, a 64-pixel app icon.
        for name in ("Logo.imageset", "Icons/Star.symbolset", "AppIcon.appiconset"):
            shutil.rmtree(os.path.join(assets_dir, name))
        pbx = os.path.join(hello, "HelloZ.xcodeproj", "project.pbxproj")
        before = open(pbx, "rb").read()
        z = Zcode(hello)
        try:
            z.run(f"open {assets_dir}")
            wait_assets(z, lambda a: a["loaded"])
            z.run("assets new color")
            edited(z, 1)
            z.run("assets color 0 red 0.8")
            edited(z, 2)
            z.run("assets select /")
            a = wait_assets(z, lambda a: a["selected_name"] == "Assets" and "row:Seed" in a["places"])
            x, y = a["places"]["row:Seed"]
            z.run(f"file-drop {x} {y} {hero2x};{orange}")
            edited(z, 3)
            z.run("assets select Brand", "assets color Dark green 0x10")
            edited(z, 4)
        finally:
            z.quit()
        assert open(pbx, "rb").read() == before, "a synchronized folder needs no project edits"
        out = os.path.join(WORK, "Assets", "actool")
        os.makedirs(out)
        result = subprocess.run(["xcrun", "actool", "--compile", out, "--platform", "iphonesimulator",
                                 "--minimum-deployment-target", "17.0",
                                 "--output-partial-info-plist", os.path.join(out, "partial.plist"),
                                 "--output-format", "human-readable-text", "--errors", "--warnings", assets_dir],
                                capture_output=True, text=True)
        assert result.returncode == 0 and "error" not in result.stdout.lower(), result.stdout + result.stderr
        info = json.loads(subprocess.run(["xcrun", "assetutil", "--info", os.path.join(out, "Assets.car")],
                                         capture_output=True, text=True, check=True).stdout)
        names = {i.get("Name") for i in info}
        assert {"Color", "Brand", "hero", "orange", "Icons/Badge", "Seed"} <= names, sorted(n for n in names if n)

    check("Asset catalog: in a synchronized project (HelloZ) nothing in project.pbxproj changes, and actool compiles what zcode wrote (new color, edited colors, dropped images)", synchronized_project_and_actool)

    def huge_catalog_stays_responsive():
        big = os.path.join(WORK, "Assets", "Big")
        shutil.copytree(os.path.join(samples, "HelloClassic"), big)
        big_catalog = os.path.join(big, "HelloClassic", "Assets.xcassets")
        image = png_bytes(256, 256, (88, 86, 214, 255))
        info = '  "info" : {\n    "author" : "xcode",\n    "version" : 1\n  }\n'
        for i in range(3000):
            folder = os.path.join(big_catalog, f"Group{i // 300}", f"Image{i:04}.imageset")
            os.makedirs(folder)
            open(os.path.join(folder, "img.png"), "wb").write(image)
            open(os.path.join(folder, "Contents.json"), "w").write(
                '{\n  "images" : [\n    {\n      "filename" : "img.png",\n      "idiom" : "universal",\n      "scale" : "1x"\n    }\n  ],\n' + info + '}\n')
        z = Zcode(big)
        try:
            started = time.time()
            z.run(f"open {big_catalog}")
            # The window answers while the catalog is read.
            slowest = 0
            while True:
                t = time.time()
                a = assets(z)
                slowest = max(slowest, time.time() - t)
                if a.get("loaded"):
                    break
                assert time.time() - started < 30, "catalog loaded"
            # And the sample's AccentColor.
            assert a["sets"] == 3001 and len(a["outline"]) == 11, (a["sets"], len(a["outline"]))
            z.run("assets expand Group5", "assets select Group5/Image1500")
            a = wait_assets(z, lambda a: a["slots"] and a["slots"][0]["thumbnail"] == "ready")
            z.run("assets select Group9")
            a = wait_assets(z, lambda a: a["thumbnails"]["decoded"] > 5 and a["selected_name"] == "Group9")
            time.sleep(1)
            a = assets(z)
            # Only the tiles on screen are decoded, not the folder's 300.
            assert a["thumbnails"]["decoded"] < 120, a["thumbnails"]
            assert a["thumbnails"]["bytes"] < 64 * 1024 * 1024, a["thumbnails"]
            print(f"    3,000 sets read in {time.time() - started:.2f} s; slowest dump meanwhile {slowest * 1000:.0f} ms; {a['thumbnails']}")
            assert slowest < 1.0, slowest
        finally:
            z.quit()

    check("Asset catalog: 3,000 image sets read off the main thread, only the thumbnails on screen decoded", huge_catalog_stays_responsive)


GALLERY_SWIFT = """import SwiftUI

struct Badge: View {
    var body: some View {
        Text("Badge")
    }
}

#Preview("Light") {
    Badge()
}

#if DEBUG
struct Badge_Previews: PreviewProvider {
    static var previews: some View {
        Badge()
    }
}
#endif
"""


def xcode_commands():
    """The commands zcode would have run to hand work to Xcode."""
    try:
        with open(XCODE_COMMANDS) as f:
            return [json.loads(l) for l in f if l.strip()]
    except FileNotFoundError:
        return []


def preview_checks(samples):
    """SwiftUI previews on a copy of HelloZ (synchronized) given a file with
    a named #Preview and a PreviewProvider: the gutter's marks, following
    edits; a mark's click and Editor ▸ Show in Xcode Canvas (⌥⌘↩), which
    save the file and hand it to Xcode. Xcode is never opened: the run's
    ZCODE_XCODE_COMMANDS records the commands instead (`xed <project>`,
    System Events waiting for its window, `xed --project … --line …`, then
    System Events turning on Editor ▸ Canvas)."""
    project = os.path.realpath(os.path.join(WORK, "Previews", "HelloZ"))
    shutil.copytree(os.path.join(samples, "HelloZ"), project)
    app = os.path.join(project, "HelloZ")
    content, counter, gallery = (os.path.join(app, n) for n in ("ContentView.swift", "Counter.swift", "Gallery.swift"))
    open(gallery, "w").write(GALLERY_SWIFT)
    xcodeproj = os.path.join(project, "HelloZ.xcodeproj")

    def previews(z):
        return z.dump()["previews"]

    def marks(z, count, timeout=30):
        s = z.wait_for(lambda s: len(s["previews"]["gutter"]) == count, timeout=timeout, interval=0.3)
        return s["previews"]

    def requested(z, before):
        """The file's xed and the canvas step, after the project's (checked)."""
        z.wait_for(lambda s: (s["previews"]["last"] or {}).get("outcome") == "recorded" and len(xcode_commands()) >= before + 4, timeout=15, interval=0.2)
        open_project, wait, xed, canvas = xcode_commands()[before:]
        assert open_project == ["/usr/bin/xed", xcodeproj], open_project
        assert wait[0] == "/usr/bin/osascript" and wait[3:] == ["HelloZ.xcodeproj", "HelloZ"], wait[3:]
        return xed, canvas

    z = Zcode(project)
    try:
        def gutter_marks():
            z.run(f"open {content}", "wait 300")
            p = marks(z, 1)
            preview_line = line_of(content, "#Preview")
            assert p["file"] == [f"Macro Preview @{preview_line}"], p
            assert [(m["line"], m["preview"]) for m in p["gutter"]] == [(preview_line, "Preview")], p
            z.run(f"open {gallery}", "wait 300")
            p = marks(z, 2)
            assert p["file"] == [f"Macro Light @{line_of(gallery, '#Preview')}", f"Provider Badge_Previews @{line_of(gallery, 'struct Badge_Previews')}"], p
            assert [m["preview"] for m in p["gutter"]] == ["Light", "Badge_Previews"], p
            z.run("screenshot " + os.path.join(WORK, "previews.png"))

        check("Previews: marks in the gutter on #Preview and PreviewProvider", gutter_marks)

        def marks_follow_edits():
            z.run(f"open {gallery}:1", "wait 300", "keys enter enter", "wait 500")
            p = marks(z, 2)
            assert [m["line"] for m in p["gutter"]] == [line_of(gallery, "#Preview") + 2, line_of(gallery, "struct Badge_Previews") + 2], p
            # A new preview at the end gets a mark of its own.
            z.run("keys cmd-down enter", "type #Preview(\"Dark\") { Badge() }", "wait 500")
            p = marks(z, 3)
            assert p["file"][-1].startswith("Macro Dark @"), p
            for _ in range(40):
                if not z.dump()["active"]["dirty"]:
                    break
                z.run("keys cmd-z")
            z.wait_for(lambda s: len(s["previews"]["file"]) == 2 and not s["active"]["dirty"], timeout=10, interval=0.3)

        check("Previews: the marks follow edits (typing stops), a typed #Preview gets one, undo removes it", marks_follow_edits)

        def click_mark():
            z.run(f"open {gallery}", "wait 300")
            p = marks(z, 2)
            before = len(xcode_commands())
            mark = next(m for m in p["gutter"] if m["preview"] == "Light")
            z.run(f"click {mark['x']} {mark['y']}")
            xed, canvas = requested(z, before)
            assert xed == ["/usr/bin/xed", "--project", xcodeproj, "--line", str(mark["line"]), gallery], xed
            assert canvas[0] == "/usr/bin/osascript" and "Canvas" in canvas[2], canvas[:2]
            assert canvas[3:] == ["Gallery.swift", "file://" + gallery], canvas[3:]
            last = previews(z)["last"]
            assert last == {"file": gallery, "line": mark["line"], "preview": "Light", "outcome": "recorded"}, last

        check("Previews: clicking a mark opens the project, then the file at that line, in Xcode, and turns on the canvas", click_mark)

        def show_in_canvas_key():
            provider_line = line_of(gallery, "struct Badge_Previews")
            inside = line_of(gallery, "static var previews") + 1
            # An unsaved edit in the provider: saved before Xcode reads the file.
            z.run(f"open {gallery}:{inside}", "wait 300", "keys cmd-right", "type .padding()", "wait 100")
            assert z.dump()["active"]["dirty"], "edited"
            before = len(xcode_commands())
            z.run("keys alt-cmd-enter")
            xed, _ = requested(z, before)
            assert xed[-3:] == ["--line", str(provider_line), gallery], xed
            assert previews(z)["last"]["preview"] == "Badge_Previews"
            assert "Badge().padding()" in open(gallery).read() and not z.dump()["active"]["dirty"], "saved first"
            # Outside a preview: the file at the cursor's line.
            body = line_of(gallery, 'Text("Badge")')
            before = len(xcode_commands())
            z.run(f"open {gallery}:{body}", "wait 300", "keys alt-cmd-enter")
            xed, _ = requested(z, before)
            assert xed[-3:] == ["--line", str(body), gallery], xed
            assert previews(z)["last"]["preview"] is None
            # A file without previews: nothing goes to Xcode.
            before = len(xcode_commands())
            z.run(f"open {counter}", "wait 500", "keys alt-cmd-enter", "wait 500")
            assert len(xcode_commands()) == before and previews(z)["last"]["line"] == body, xcode_commands()[before:]

        check("Show in Xcode Canvas (⌥⌘↩): the preview at the cursor, else the cursor's line, the file saved first; nothing for a file without previews", show_in_canvas_key)

        def marks_in_the_assistant():
            z.run(f"open {gallery}", "wait 300", "keys ctrl-cmd-t", "wait 500")
            p = marks(z, 2)
            assert [m["preview"] for m in p["gutter"]] == ["Light", "Badge_Previews"], p
            z.run("keys ctrl-shift-cmd-w", "wait 300")

        check("Previews: marks in a second pane's view of the file", marks_in_the_assistant)
    finally:
        z.quit()


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
        pid = running_app_pid(z)
        z.run("keys cmd-.")
        # Gone, and reaped: Stop leaves nothing behind.
        process_gone(pid)

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
        pid = running_app_pid(z)
        z.run("console p ProcessInfo.processInfo.processName")
        out = console_output(z, "p ProcessInfo.processInfo.processName", timeout=120)
        assert any('"HelloClassic"' in l for l in out), out
        # The log message prints and the app runs on without stopping.
        z.run("keys ctrl-cmd-y")
        s = console_has(z, "launched with 1 argument(s), hit 1", timeout=60)
        console_has(z, "arguments: ", timeout=30)
        assert z.dump()["debugger"]["state"] == "Running", z.dump()["debugger"]
        z.run("keys cmd-.")
        process_gone(pid)
        z.run("action zcode::DeleteAllBreakpoints")

    check("Mac app under lldb-dap: breakpoint stop, LLDB console, auto-continuing log message", debug_mac_app)

    def stop_at_breakpoint():
        # Paused at a breakpoint, the app is held by debugserver; Stop still ends it.
        content = next(p for p in (os.path.join(app, "Views", "ContentView.swift"), os.path.join(app, "ContentView.swift"))
                       if os.path.exists(p))
        launched = line_of(content, 'print("HelloClassic launched")')
        z.run(f"open {content}:{launched}", "wait 300", "keys cmd-\\")
        z.run("action zcode::ClearConsole", "keys cmd-r")
        stopped(z, variables=False)
        pid = running_app_pid(z)
        z.run("keys cmd-.")
        process_gone(pid)
        assert z.dump()["run_pid"] is None
        z.run("action zcode::DeleteAllBreakpoints")

    check("Stop (⌘.) ends a Mac app paused at a breakpoint, leaving no process", stop_at_breakpoint)

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

    def scale_instrumentation():
        # What scripts/bench.py reads on a big project, checked here on a small one.
        s = z.wait_for(lambda s: s["timings"].get("tree"), timeout=30, interval=0.3)
        t = s["timings"]
        assert t["scan"]["count"] == s["files"] > 0 and 0 < t["scan"]["ms"] < 1000 and t["tree"]["at_ms"] > t["scan"]["ms"], t
        status = lambda: json.loads(z.run("bench status")[-1])
        b = status()
        assert b["files"] == s["files"] and b["memory"]["resident_mb"] > 0 and b["timings"]["scan"] == t["scan"], b
        frame = json.loads(z.run("bench frame")[-1])
        assert 0 < frame["to_frame_ms"] < frame["drawn_ms"] < 1000 and 0 < frame["work_ms"] <= frame["drawn_ms"], frame
        # Open Quickly shown and typed into, its matching timed over every file.
        shown = json.loads(z.run("bench action zcode::OpenQuickly")[-1])
        assert shown["drawn_ms"] > 0 and status()["modal_open"], (shown, status())
        typed = json.loads(z.run("type-latency Gre")[-1])
        assert typed["keys"] == 3 and typed["median_ms"] <= typed["drawn_median_ms"] and typed["drawn_p95_ms"] <= typed["drawn_max_ms"], typed
        assert 0 < typed["work_median_ms"] <= typed["work_max_ms"] < 1000, typed
        assert status()["timings"]["open_quickly"]["count"] == s["files"], status()["timings"]
        z.run("keys escape", "wait 200")
        # A search timed with its matches counted; the navigator's rows laid out only on screen.
        keys = json.loads(z.run("bench keys cmd-shift-f")[-1])
        assert keys["drawn_ms"] > 0 and status()["navigator"] == "Find", (keys, status())
        z.run("type greeting", "keys enter")
        b = z.wait_for(lambda s: s["find_results"] > 0 and s["timings"].get("find"), timeout=20, interval=0.3)
        assert b["timings"]["find"]["count"] == b["find_results"] >= 4 and b["timings"]["find"]["ms"] < 1000, b["timings"]
        assert json.loads(z.run("bench frame")[-1])["drawn_ms"] < 1000
        print(f"    scan {t['scan']['ms']:.1f} ms for {s['files']} files, tree {t['tree']['ms']:.1f} ms, search {b['timings']['find']['ms']:.1f} ms, a frame's work {frame['work_ms']:.1f} ms")
        z.run("keys cmd-1", "wait 200")

    check("Scale instrumentation: scan, tree, search and Open Quickly timings in dump and bench status; bench frame / action / keys; type-latency's drawn times", scale_instrumentation)
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
    changes = os.path.join(WORK, "lsp-changes.log")

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
        # Editor ▸ Show Callers…: a menu at the cursor; choosing report() opens the call.
        z.run(f"open {shapes}:{total_line}", keys_right(6), "action zcode::ShowCallers")
        z.wait_for(lambda s: s["breakpoints"]["menu_open"], timeout=15, interval=0.3)
        z.run("keys down", "keys enter", "wait 500")
        s = z.dump()
        assert not s["breakpoints"]["menu_open"] and s["active"]["line"] == line_of(shapes, "total(of: [sq])"), s["active"]["line"]
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
        check("Find Call Hierarchy (⌃⇧⌘H) from the focused pane: callers, callees, opening calls and declarations; Show Callers…", call_hierarchy_from_the_focused_pane)
        check("Find Ancestor / Descendent / Conforming Types in the Find navigator", type_hierarchy)
        check("Typing in a 6,000-line Swift file with hints and semantic tokens stays under a frame", typing_latency)
    finally:
        z.run("inlay-hints " + ("on" if was_on else "off"))
        z.quit()

    def bounded_traffic():
        for log in (traffic, changes):
            if os.path.exists(log):
                os.remove(log)
        z = Zcode(project, {"ZCODE_SOURCEKIT_LSP": proxy, "ZCODE_LSP_PROXY_LOG": traffic, "ZCODE_LSP_PROXY_CHANGES": changes})
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
            # The server's copy is the editor's text: the typing went as the
            # editor's own edits. Then one letter typed over two selections
            # 21 lines apart is two one-letter changes, not one range
            # spanning them (what comparing the texts would send).
            suffix = "/Sources/ZKit/Big.swift"
            sent, typed = replay_lsp_changes(changes, suffix)
            assert typed and all("range" in c for c in typed), typed[:3]
            mark = len(typed)
            z.run(f"open {big}:{line_of(big, 'name.uppercased()')}", keys_right(26),
                  *["action zcode::SelectNextOccurrence"] * 2, "type Q", "wait 1500")
            z.run(f"editor text {os.path.join(WORK, 'big-editor.txt')}")
            sent, typed = replay_lsp_changes(changes, suffix)
            assert sent == open(os.path.join(WORK, "big-editor.txt")).read(), "the server's text differs from the editor's"
            assert sent.count("name.Q()") == 2, sent.count("name.Q()")
            assert [c["text"] for c in typed[mark:]] == ["Q"] * 2, typed[mark:]
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

    check("Requests stay bounded: typing and scrolling ask for tokens and hints a few times, not per keystroke; the server's copy follows the editor's own edits", bounded_traffic)
    check("A server without these features: nothing asked for, commands say so, the rest works", without_the_features)


FAKE_PRETTIER = """#!/usr/bin/env python3
# A stand-in for a web app's Prettier: records its arguments, then formats
# stdin by collapsing runs of spaces and ending statements with semicolons.
import os, re, sys
with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "prettier-args.log"), "a") as f:
    f.write(" ".join(sys.argv[1:]) + "\\n")
text = sys.stdin.read()
lines = [re.sub(r"(?<=\\S) {2,}", " ", l) for l in text.split("\\n")]
lines = [l + ";" if re.match(r"^\\s*(const|let) .*[^;{,(]$", l) else l for l in lines]
sys.stdout.write("\\n".join(lines))
"""

EXTRA_TS = "/** Everything the page has counted. */\nexport const totalTaps = 0\n"


def web_language_checks(samples):
    """A Swift package with a Vite app in web/ (samples/HelloWeb): the web's
    language servers, installed by zcode with npm into a folder shared by
    runs (ZCODE_LANGUAGE_SERVERS_DIR), rooted at web/ next to
    sourcekit-lsp for the Swift; TypeScript, HTML, CSS, JSON and Vue; Emmet,
    tags closed as typed, comments, Prettier (a stand-in in node_modules),
    settings and Restart Language Servers. With --network, ESLint and
    Tailwind on the app's own packages too."""
    npm = shutil.which("npm") or os.path.expanduser("~/.vite-plus/js_runtime/node/24.18.0/bin/npm")
    if not os.path.exists(npm):
        print("  (skipped: npm isn't installed)")
        return
    # zcode reports paths as they really are (/private/var/…).
    project = os.path.realpath(os.path.join(WORK, "HelloWeb"))
    shutil.copytree(os.path.join(samples, "HelloWeb"), project, ignore=shutil.ignore_patterns("node_modules", ".build", "dist"))
    web = os.path.join(project, "web")
    src = os.path.join(web, "src")
    main_ts, counter_ts, extra_ts = (os.path.join(src, n) for n in ("main.ts", "counter.ts", "extra.ts"))
    index, style, greeting = os.path.join(web, "index.html"), os.path.join(src, "style.css"), os.path.join(src, "Greeting.vue")
    package_json, main_swift = os.path.join(web, "package.json"), os.path.join(project, "Sources", "HelloWeb", "main.swift")
    with open(extra_ts, "w") as f:
        f.write(EXTRA_TS)
    bin_dir = os.path.join(web, "node_modules", ".bin")
    os.makedirs(bin_dir)
    prettier = os.path.join(bin_dir, "prettier")
    with open(prettier, "w") as f:
        f.write(FAKE_PRETTIER)
    os.chmod(prettier, 0o755)
    library = os.path.join(web, "node_modules", "left-pad", "index.d.ts")
    os.makedirs(os.path.dirname(library))
    with open(library, "w") as f:
        f.write("export declare function leftPad(s: string, n: number): string\n")
    servers_dir = os.environ.get("ZCODE_LANGUAGE_SERVERS_DIR") or os.path.join(tempfile.gettempdir(), "zcode-e2e-language-servers")
    z = Zcode(project, {"ZCODE_LANGUAGE_SERVERS_DIR": servers_dir})

    def servers(s):
        return {x["id"]: x for x in s["language_servers"]}

    def ready(*ids, timeout=400):
        return z.wait_for(lambda s: all(servers(s).get(i, {}).get("status") == "Ready" for i in ids), timeout=timeout, interval=1)

    def menu(timeout=30):
        return z.wait_for(lambda s: s["active"]["completion_menu_open"], timeout=timeout, interval=0.3)["active"]["completion_items"]

    def text():
        return z.dump()["active"]["text"]

    def revert(path):
        for _ in range(12):
            if not z.dump()["active"]["dirty"]:
                return
            z.run("keys cmd-z", "wait 150")
        assert not z.dump()["active"]["dirty"], f"{os.path.basename(path)} still edited"

    def servers_per_web_app():
        z.run(f"open {main_ts}:1")
        s = ready("vtsls")
        vtsls = servers(s)["vtsls"]
        assert vtsls["root"] == web and vtsls["documents"] == 1, vtsls
        assert os.path.isdir(os.path.join(servers_dir, "vtsls-language-server", "node_modules")), "installed in zcode's folder"
        assert s["lsp"] == "Ready", s["lsp"]
        # The Swift beside it has sourcekit-lsp, for the whole project.
        z.run(f"open {main_swift}:1")
        s = ready("sourcekit-lsp", timeout=300)
        assert servers(s)["sourcekit-lsp"]["root"] == project, servers(s)
        assert servers(s)["vtsls"]["documents"] == 1, "a Swift file isn't vtsls's"

    def typescript_completion():
        z.run(f"open {main_ts}:{line_of(main_ts, 'render()')}", "keys end", "keys enter", "type counter.inc")
        assert "increment" in menu(), "a member of Counter"
        z.run("keys enter", "wait 400")
        line = z.dump()["active"]["line_text"]
        assert line.strip().startswith("counter.increment"), line
        revert(main_ts)

    def typescript_diagnostics():
        z.run(f"open {main_ts}:{line_of(main_ts, 'render()')}", "keys end", "keys enter", "type const wrong: number = 'x'")
        s = z.wait_for(lambda s: any("main.ts" in i and "not assignable" in i for i in s["issues"]), timeout=60, interval=0.5)
        revert(main_ts)
        z.wait_for(lambda s: not any("main.ts" in i for i in s["issues"]), timeout=60, interval=0.5)

    def auto_import():
        z.run(f"open {main_ts}:{line_of(main_ts, 'render()')}", "keys end", "keys enter", "type const all = totalTa")
        assert "totalTaps" in menu()
        z.run("keys enter")
        z.wait_for(lambda s: "from './extra'" in s["active"]["text"], timeout=20, interval=0.3)
        assert "import { totalTaps } from './extra'" in text().splitlines()[:3], text().splitlines()[:3]
        revert(main_ts)

    def definition_across_files():
        z.run(f"open {main_ts}:{line_of(main_ts, 'new Counter')}", "keys home", keys_right(column_of(main_ts, "Counter(") + 2), "keys ctrl-cmd-j")
        s = z.wait_for(lambda s: s["active"]["path"] == counter_ts, timeout=30, interval=0.3)
        # `new Counter(1)` is a call of its constructor.
        assert "constructor(" in s["active"]["line_text"], s["active"]["line_text"]

    def html_completion_and_tags():
        z.run(f"open {index}:{line_of(index, '<div id=')}")
        s = ready("vscode-html-language-server")
        z.run("keys end", "keys enter", "type <sec")
        assert "section" in menu(), "the HTML server's tags"
        z.run("keys escape", "type tion class=\"hero\">", "wait 200")
        assert z.dump()["active"]["line_text"].strip() == '<section class="hero"></section>', "closed as its > was typed"
        assert servers(s)["vscode-html-language-server"]["root"] == web
        revert(index)

    def emmet_markup():
        z.run(f"open {index}:{line_of(index, '<div id=')}", "keys end", "keys enter", "type ul.list>li*3")
        items = menu()
        assert items[0] == "ul.list>li*3", items
        z.run("keys enter", "type One", "keys tab", "type Two", "wait 200")
        t = text()
        assert '<ul class="list">\n        <li>One</li>\n        <li>Two</li>\n        <li></li>\n    </ul>' in t, t[t.find("<ul"):][:200]
        revert(index)
        # ⌃⌥E expands without the menu; nothing to expand says so.
        z.run(f"open {index}:{line_of(index, '<div id=')}", "keys end", "keys enter", "type nav>a*2", "keys escape", "keys ctrl-alt-e", "wait 200")
        assert '<nav><a href=""></a><a href=""></a></nav>' in text(), text()
        revert(index)

    def css_completion_emmet_and_comments():
        z.run(f"open {style}:{line_of(style, 'padding')}")
        ready("vscode-css-language-server")
        z.run("keys end", "keys enter", "type marg")
        assert "margin" in menu()
        z.run("keys escape")
        revert(style)
        z.run(f"open {style}:{line_of(style, 'padding')}", "keys end", "keys enter", "type m10-20")
        assert menu()[0] == "m10-20"
        z.run("keys enter", "wait 200")
        assert z.dump()["active"]["line_text"].strip() == "margin: 10px 20px;", z.dump()["active"]["line_text"]
        z.run("keys cmd-/", "wait 200")
        assert z.dump()["active"]["line_text"].strip() == "/* margin: 10px 20px; */"
        revert(style)
        z.run(f"open {index}:{line_of(index, '<div id=')}", "keys cmd-/", "wait 200")
        assert z.dump()["active"]["line_text"].strip() == '<!-- <div id="app"></div> -->'
        revert(index)

    def json_diagnostics():
        z.run(f"open {package_json}:2")
        ready("vscode-json-language-server")
        z.run("keys end", "type  oops")
        z.wait_for(lambda s: any("package.json" in i for i in s["issues"]), timeout=60, interval=0.5)
        revert(package_json)
        z.wait_for(lambda s: not any("package.json" in i for i in s["issues"]), timeout=60, interval=0.5)

    def vue_through_typescript():
        line = line_of(greeting, "{{ greeting }}")
        z.run(f"open {greeting}:{line}")
        s = ready("vue-language-server", "vtsls")
        assert [x["id"] for x in s["language_servers"] if x["documents"] and x["id"] in ("vue-language-server", "vtsls")], s["language_servers"]
        # The template's expressions are TypeScript's (Vue's plugin in vtsls,
        # through zcode's tsserver/request relay).
        for _ in range(10):
            z.run("keys home", keys_right(column_of(greeting, "{{ gree") + len("{{ gree")), "keys ctrl-space", "wait 1500")
            s = z.dump()
            if s["active"]["completion_menu_open"]:
                break
        assert "greeting" in s["active"]["completion_items"], s["active"]["completion_items"]
        z.run("keys escape")

    def prettier_formats():
        z.run(f"open {counter_ts}:1", "keys home", "type const   spaced  =  1", "keys enter", "keys ctrl-shift-i")
        z.wait_for(lambda s: s["active"]["text"].startswith("const spaced = 1;\n/**"), timeout=20, interval=0.3)
        with open(os.path.join(bin_dir, "prettier-args.log")) as f:
            assert f"--stdin-filepath {counter_ts}" in f.read()
        revert(counter_ts)

    def libraries_are_read_only():
        z.run(f"open {library}:1")
        s = z.wait_for(lambda s: s["active"]["path"] == library, timeout=10, interval=0.3)
        assert s["active"]["read_only"], "node_modules is shown, never edited"

    def a_server_turned_off_and_restarts():
        z.run("settings set lsp.vscode-css-language-server.enabled false", "wait 800")
        z.run(f"open {style}:1")
        s = z.wait_for(lambda s: "vscode-css-language-server" not in [x["id"] for x in s["language_servers"] if x["documents"]], timeout=20, interval=0.5)
        z.run("settings set lsp.vscode-css-language-server.enabled true", "wait 800")
        ready("vscode-css-language-server", timeout=60)
        before = {x["id"]: x["pid"] for x in z.dump()["language_servers"]}
        z.run(f"open {main_ts}:1", "action zcode::RestartLanguageServers")
        s = z.wait_for(lambda s: all(x["status"] == "Ready" for x in s["language_servers"]), timeout=180, interval=1)
        assert all(x["pid"] != before.get(x["id"]) for x in s["language_servers"]), "each one is a new process"

    try:
        check("Web servers per web app (vtsls installed by zcode, rooted at web/), sourcekit-lsp for the Swift beside it", servers_per_web_app)
        check("TypeScript: completions", typescript_completion)
        check("TypeScript: live diagnostics in the Issue navigator", typescript_diagnostics)
        check("TypeScript: a completion's auto-import", auto_import)
        check("TypeScript: Jump to Definition in another file", definition_across_files)
        check("HTML: the server's completions; a tag closed as its > is typed", html_completion_and_tags)
        check("Emmet in HTML: a completion with Tab stops, ⌃⌥E", emmet_markup)
        check("CSS: completions, Emmet's m10-20, ⌘/ as /* */ (and <!-- --> in HTML)", css_completion_emmet_and_comments)
        check("JSON: the server's diagnostics", json_diagnostics)
        check("Vue: the template's expressions completed through vtsls", vue_through_typescript)
        check("Format File with the web app's Prettier (a stand-in)", prettier_formats)
        check("node_modules files open read-only", libraries_are_read_only)
        check("lsp.<id>.enabled turns a server off and on; Restart Language Servers", a_server_turned_off_and_restarts)
    finally:
        z.quit()

    if "--network" not in sys.argv:
        return

    def eslint_and_tailwind():
        app = os.path.join(WORK, "WebTools")
        shutil.copytree(project, app, ignore=shutil.ignore_patterns("node_modules", ".build"))
        web = os.path.join(app, "web")
        subprocess.run([npm, "install", "--no-audit", "--no-fund", "--loglevel=error", "-D", "eslint", "@eslint/js", "typescript-eslint", "tailwindcss@4", "@tailwindcss/vite"],
                       cwd=web, check=True, stdout=subprocess.DEVNULL)
        with open(os.path.join(web, "eslint.config.js"), "w") as f:
            f.write("import js from '@eslint/js'\nimport tseslint from 'typescript-eslint'\n\nexport default tseslint.config(\n"
                    "  { extends: [js.configs.recommended, ...tseslint.configs.recommended], files: ['**/*.ts'], rules: { 'prefer-const': 'error' } },\n)\n")
        with open(os.path.join(web, "src", "style.css"), "a") as f:
            f.write('\n@import "tailwindcss";\n')
        main_ts = os.path.join(web, "src", "main.ts")
        z = Zcode(app, {"ZCODE_LANGUAGE_SERVERS_DIR": servers_dir})
        try:
            z.run(f"open {main_ts}:{line_of(main_ts, 'render()')}")
            z.wait_for(lambda s: all(servers(s).get(i, {}).get("status") == "Ready" for i in ("vtsls", "eslint", "tailwindcss-language-server")), timeout=400, interval=1)
            z.run("keys end", "keys enter", "type let never = 1")
            z.wait_for(lambda s: any("Use 'const' instead" in i for i in s["issues"]), timeout=60, interval=0.5)
            # Tailwind's classes in a class attribute of the page.
            index = os.path.join(web, "index.html")
            z.run(f"open {index}:{line_of(index, '<div id=')}", "keys end", "keys enter", 'type <p class="bg-sky-')
            for _ in range(10):
                z.run("keys ctrl-space", "wait 1500")
                s = z.dump()
                if s["active"]["completion_menu_open"]:
                    break
            assert "bg-sky-50" in s["active"]["completion_items"], s["active"]["completion_items"][:5]
        finally:
            z.quit()

    check("ESLint's diagnostics (pulled) and Tailwind's classes, from the app's own packages", eslint_and_tailwind)


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
        # Inline: deleted lines are blocks under the line before them, or
        # above the first line (the deleted first line, then the modified
        # line's old text under the new first line).
        z.run("action zcode::InlineComparison", "wait 300")
        d = g()["diff"]
        assert not d["side_by_side"] and d["current_blocks"] == [[1, 1, True], [1, 1, False]], d["current_blocks"]
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


def multibuffer_checks(samples):
    """Multibuffers on a copy of ZKit made a git repository, with 400 text
    files that match a query: Find results in a tab edited in place, Replace
    All as one undo step across files, opening the files and showing more
    lines, change bars and breakpoints on excerpt lines, the issues with
    their messages, a rename's changes, and a search over many files that
    opens a page at a time."""
    project = os.path.realpath(os.path.join(WORK, "MultiKit"))
    shutil.copytree(os.path.join(samples, "ZKit"), project, ignore=shutil.ignore_patterns(".build"))
    with open(os.path.join(project, ".gitignore"), "w") as f:
        f.write(".build/\n.swiftpm/\n")
    notes = os.path.join(project, "Notes")
    os.makedirs(notes)
    for i in range(400):
        with open(os.path.join(notes, f"note{i:03}.txt"), "w") as f:
            f.write(f"note {i}\nthe needle {i}\nend\n")
    init_repo(project)
    git(project, "add", "-A")
    git(project, "commit", "-q", "-m", "Initial commit")
    greeter = os.path.join(project, "Sources", "ZKit", "Greeter.swift")
    main_swift = os.path.join(project, "Sources", "zhello", "main.swift")
    originals = {p: open(p).read() for p in (greeter, main_swift)}
    greeting_line = line_of(greeter, "public func greeting")
    z = Zcode(project)

    def mb(s=None):
        return (s or z.dump())["multibuffer"]["active"]

    def back():
        """Shows the multibuffer's tab again."""
        tabs = z.dump()["panes"]["list"][0]["tabs"]
        ix = next(i for i, t in enumerate(tabs) if t.startswith(("Find", "Issues", "Rename")))
        z.run(f"pane tab 0 {ix}", "wait 300")

    def row_of(m, file, line):
        """The window point of a file's line in the multibuffer, in its text."""
        n = next(n for n in m["numbers"] if n["file"] == file and n["line"] == line)
        return n["x"] + 300, n["y"]

    def find(query):
        z.run("keys cmd-shift-f", "wait 200", f"type {query}", "keys enter")
        assert not z.dump()["find_replacing"]
        z.wait_for(lambda s: s["find_results"] > 0, timeout=20, interval=0.3)

    def find_results():
        find("greeting")
        found = z.dump()["find_results"]
        z.run("action zcode::ShowFindResultsInEditor", "wait 500")
        s = z.dump()
        m = mb(s)
        assert s["panes"]["list"][0]["active"] == "Find “greeting”", s["panes"]["list"][0]
        assert m["matches"] == found and m["pending_files"] == 0, (m["matches"], found)
        assert {"Greeter.swift", "main.swift", "GreeterTests.swift"} <= set(m["files"]), m["files"]
        headers = [h for h in m["headers"] if h["key"] == "multibuffer-headers"]
        assert len(headers) == len(m["excerpts"]), (headers, m["excerpts"])
        # The gutter numbers are each line's in its file.
        assert any(n["file"] == "Greeter.swift" and n["line"] == greeting_line for n in m["numbers"]), m["numbers"]
        assert m["text"].startswith(open(os.path.join(project, "Package.swift")).read().splitlines()[0]) or m["excerpts"][0]["file"] != "Package.swift"
        z.run("screenshot " + os.path.join(WORK, "multibuffer-find.png"))

    def edits_both_ways():
        # Typing in Swift excerpts, against the file's own tab.
        x, y = row_of(mb(), "Greeter.swift", greeting_line)
        z.run(f"click {x} {y}", "keys cmd-right")
        excerpt = json.loads(z.run("type-latency abcdefghijklmnopqrstuvwxyz")[-1])
        z.run("keys cmd-z", "wait 300")
        z.run(f"open {greeter}:{greeting_line}", "keys cmd-right")
        tab = json.loads(z.run("type-latency abcdefghijklmnopqrstuvwxyz")[-1])
        z.run("keys cmd-z", "wait 300")
        print(f"      typing, median / p95: excerpts {excerpt['median_ms']:.1f} / {excerpt['p95_ms']:.1f} ms, the file's tab {tab['median_ms']:.1f} / {tab['p95_ms']:.1f} ms")
        assert excerpt["median_ms"] < 16 and not z.dump()["active"]["dirty"], excerpt
        back()
        z.run(f"click {x} {y}", "keys cmd-right", "type  // here", "wait 300")
        m = mb()
        assert "Greeter.swift" in m["dirty"] and "// here" in m["text"], m["dirty"]
        assert open(greeter).read() == originals[greeter], "nothing saved"
        # The file's tab has the edit; typing there shows in the excerpt.
        z.run(f"open {greeter}:{greeting_line}", "keys cmd-right", "type !", "wait 200")
        assert "// here!" in z.dump()["active"]["text"]
        back()
        assert "// here!" in mb()["text"]
        # The file's own ⌘Z undoes both, the multibuffer's edit included.
        z.run(f"open {greeter}:{greeting_line}", "keys cmd-z", "keys cmd-z", "wait 300")
        s = z.dump()
        assert "// here" not in s["active"]["text"] and not s["active"]["dirty"], s["active"]["text"][:200]
        back()
        assert "// here" not in mb()["text"] and not mb()["dirty"]

    def replace_all_one_step():
        z.run("keys alt-shift-cmd-f", "wait 200", "type greeting", "keys enter")
        z.wait_for(lambda s: s["find_results"] > 0 and s["find_replacing"], timeout=20, interval=0.3)
        z.run("screenshot " + os.path.join(WORK, "multibuffer-replace-navigator.png"))
        # The Replace field's Replace All.
        z.run("multibuffer replace salute", "wait 500")
        m = mb()
        assert m["dirty"] and len(m["dirty"]) == len(m["files"]) >= 3, m["dirty"]
        assert "salute" in m["text"] and "greeting(" not in m["text"] and m["undo"] == 1, m["undo"]
        assert open(greeter).read() == originals[greeter], "Replace All leaves the files unsaved"
        z.run("screenshot " + os.path.join(WORK, "multibuffer-replace.png"))
        z.run("keys cmd-z", "wait 400")
        m = mb()
        assert not m["dirty"] and "salute" not in m["text"] and (m["undo"], m["redo"]) == (0, 1), (m["dirty"], m["undo"], m["redo"])
        z.run("keys cmd-shift-z", "wait 400")
        files = len(mb()["files"])
        assert len(mb()["dirty"]) == files
        # A file's own ⌘Z takes its part back; the multibuffer's ⌘Z the rest.
        z.run(f"open {greeter}:{greeting_line}", "keys cmd-z", "wait 300")
        assert "func greeting" in z.dump()["active"]["text"]
        back()
        assert "Greeter.swift" not in mb()["dirty"] and len(mb()["dirty"]) == files - 1
        z.run("keys cmd-z", "wait 400")
        assert not mb()["dirty"]
        # Redone together, then saved from the tab.
        z.run("keys cmd-shift-z", "wait 400")
        assert len(mb()["dirty"]) == files, mb()["dirty"]
        z.run("keys cmd-s", "wait 500")
        assert not mb()["dirty"] and "func salute" in open(greeter).read() and "salute(" in open(main_swift).read()

    def open_and_expand():
        m = mb()
        x, y = row_of(m, "main.swift", line_of(main_swift, "salute("))
        z.run(f"click {x} {y}", "keys alt-enter", "wait 400")
        s = z.dump()
        assert s["active"]["path"] == main_swift and s["active"]["line"] == line_of(main_swift, "salute("), s["active"]["path"]
        back()
        header = next(h for h in mb()["headers"] if h["key"] == "multibuffer-headers" and h["row"] > 0)
        first_rows = mb()["excerpts"]
        z.run(f"click {header['x'] + 250} {header['y']}", "wait 400")
        assert z.dump()["active"] is not None and z.dump()["panes"]["list"][0]["active"] != "Find “greeting”"
        back()
        n = mb()["numbers"][0]
        z.run(f"click {n['x']} {n['y']}", "wait 400")
        s = z.dump()
        assert s["active"]["line"] == n["line"] and s["panes"]["list"][0]["active"] == n["file"], (s["active"]["line"], n)
        back()
        # ⇧↩ shows three more lines around the cursor's excerpt.
        x, y = row_of(mb(), "main.swift", line_of(main_swift, "salute("))
        before = next(e for e in mb()["excerpts"] if e["file"] == "main.swift")
        z.run(f"click {x} {y}", "keys shift-enter", "wait 300")
        after = next(e for e in mb()["excerpts"] if e["file"] == "main.swift")
        assert after["lines"][0] <= max(1, before["lines"][0] - 3) and after["lines"][1] >= before["lines"][1] + 1, (before, after)
        assert "\n\n" not in mb()["text"][:0]
        # A header's ↕ does the same for its excerpt.
        header = next(h for h in mb()["headers"] if h["key"] == "multibuffer-headers")
        count = len(mb()["text"].splitlines())
        z.run(f"click {header['x'] + 6} {header['y']}", "wait 300")
        assert len(mb()["text"].splitlines()) > count, first_rows

    def gutter_marks():
        # The saved rename is a change against HEAD; its lines get bars.
        z.wait_for(lambda s: any(lane == "multibuffer-changes" for lane, _ in mb(s)["painted_gutter"]), timeout=20, interval=0.5)
        # A breakpoint set in the file's tab shows on its excerpt line, and
        # toggles there.
        line = line_of(greeter, "public func salute")
        z.run(f"open {greeter}:{line}", "keys cmd-\\", "wait 200", "wait 200")
        back()
        m = mb()
        marks = [row for lane, row in m["painted_gutter"] if lane == "multibuffer-breakpoints"]
        assert marks, m["painted_gutter"]
        n = next(n for n in m["numbers"] if n["row"] == marks[0])
        assert (n["file"], n["line"]) == ("Greeter.swift", line), n
        z.run(f"click {n['x']} {n['y']}", "wait 300")
        bps = z.dump()["breakpoints"]["list"]
        assert [b["enabled"] for b in bps if b["title"] == f"Greeter.swift:{line}"] == [False], bps
        z.run("screenshot " + os.path.join(WORK, "multibuffer-gutter.png"))

    def issues_in_editor():
        line = line_of(greeter, "public let name")
        before = open(greeter).read()
        z.run(f"open {greeter}:{line}", "keys cmd-right", "type + oops", "keys cmd-s", "keys cmd-b")
        s = z.wait_for(lambda s: s["phase"].startswith("Finished"), timeout=300)
        assert "success: false" in s["phase"], s["phase"]
        z.run("action zcode::ShowIssuesInEditor", "wait 500")
        s = z.dump()
        m = mb(s)
        assert s["panes"]["list"][0]["active"] == "Issues", s["panes"]["list"][0]
        assert any(e["file"] == "Greeter.swift" and e["lines"][0] <= line <= e["lines"][1] for e in m["excerpts"]), m["excerpts"]
        assert any(row == line - m["excerpts"][0]["lines"][0] + 1 for row, _ in m["messages"]) and m["diagnostics"], m["messages"]
        z.run("screenshot " + os.path.join(WORK, "multibuffer-issues.png"))
        # Fixed in the tab itself.
        x, y = row_of(m, "Greeter.swift", line)
        z.run(f"click {x} {y}", "keys cmd-right", "keys backspace backspace backspace backspace backspace backspace", "keys cmd-s", "wait 300")
        assert open(greeter).read() == before, ("fixed and saved from the issues tab", open(greeter).read(), mb()["text"])

    def rename_preview():
        line = greeting_line
        z.run(f"open {greeter}:{line}")
        z.wait_for(lambda s: s["lsp"] == "Ready" and not s["lsp_progress"] and not s["indexing"], timeout=400)
        column = open(greeter).read().splitlines()[line - 1].index("greeting") + 2
        for _ in range(5):
            z.run(f"open {greeter}:{line}", keys_right(column), "keys f2", "wait 300", "keys cmd-a", "type greet", "wait 300", "keys enter")
            try:
                s = z.wait_for(lambda s: s["panes"]["list"][0]["active"].startswith("Rename"), timeout=20, interval=0.5)
                # Until the index has every file, a rename finds only this one.
                if len(mb(s)["files"]) >= 3:
                    break
                z.run("keys cmd-z", "keys cmd-w", "wait 300")
            except TimeoutError:
                pass
            time.sleep(5)
        m = mb(s)
        assert m["title"] == "Rename “greeting” to “greet”" and m["undo"] == 1, m["title"]
        assert len(m["files"]) >= 3 and set(m["dirty"]) == set(m["files"]) and m["matches"] >= 4, m
        z.run("screenshot " + os.path.join(WORK, "multibuffer-rename.png"))
        z.run("keys cmd-z", "wait 500")
        m = mb()
        assert not m["dirty"] and "greet(" not in m["text"], m["dirty"]
        z.run("keys cmd-w", "wait 300")

    def many_files_a_page_at_a_time():
        find("needle")
        assert z.dump()["find_results"] == 400
        z.run("multibuffer find", "wait 300")
        m = mb()
        # A page, or two when one doesn't fill two screens.
        assert len(m["files"]) in (25, 50) and m["pending_files"] == 400 - len(m["files"]), (m["pending_files"], len(m["files"]))
        shown = len(m["files"])
        assert m["last_page_ms"] < 500, m["last_page_ms"]
        print(f"      first page of 25 files: {m['last_page_ms']:.1f} ms")
        # Scrolling to the end adds the next pages.
        z.run("keys cmd-down", "wait 400", "keys cmd-down", "wait 400")
        assert len(mb()["files"]) > shown, len(mb()["files"])
        z.run("multibuffer all", "wait 300")
        m = mb()
        assert m["pending_files"] == 0 and len(m["files"]) == 400 and m["matches"] == 400, (len(m["files"]), m["matches"])
        # Typing in it stays fast.
        x, y = row_of(mb(), m["numbers"][1]["file"], 2)
        z.run(f"click {x} {y}", "keys cmd-right")
        stats = json.loads(z.run("type-latency abcdefghijklmnopqrstuvwxyz")[-1])
        print(f"      typing in 400 plain-text excerpts: median {stats['median_ms']:.1f} ms, p95 {stats['p95_ms']:.1f} ms")
        assert stats["median_ms"] < 16 and mb()["dirty"], stats
        z.run("keys cmd-z", "wait 300")
        assert not mb()["dirty"]
        # Closed, it lets go of the files only it had open.
        assert sum(b.startswith("note") for b in z.dump()["buffers"]) == 400
        z.run("keys cmd-w", "wait 300")
        s = z.dump()
        assert not any(b.startswith("note") for b in s["buffers"]), s["buffers"][:10]

    try:
        check("Multibuffer: Rename (sourcekit-lsp) shows the places it changed, unsaved; one ⌘Z undoes it", rename_preview)
        check("Multibuffer: Find in Workspace's results in a tab, a header per file, each line's number in its file", find_results)
        check("Multibuffer: typing in an excerpt edits the file (unsaved); the file's edits show there; the file's ⌘Z undoes both", edits_both_ways)
        check("Multibuffer: Find and Replace in Workspace (⌥⇧⌘F) Replace All is one ⌘Z across files, each file's ⌘Z agrees; ⌘S saves", replace_all_one_step)
        check("Multibuffer: ⌥↩, a header and a line number open the file there; ⇧↩ and a header's ↕ show more lines", open_and_expand)
        check("Multibuffer: change bars and breakpoints on excerpt lines; a breakpoint toggles there", gutter_marks)
        check("Multibuffer: Show Issues in Editor, each message under its line, fixed in place", issues_in_editor)
        check("Multibuffer: a search over 400 files opens a page of 25 and adds more as it scrolls; typing stays fast", many_files_a_page_at_a_time)
    finally:
        z.quit()


def terminal_checks(samples):
    """The integrated terminal on a copy of ZKit, with `zsh -f` as the shell
    (ZCODE_TERMINAL_SHELL) so the user's configuration can't change what it
    prints: the shell in the project's folder, keys, selection and ⌘C,
    scrollback, links, resizing, several terminals, a flood of output
    while typing elsewhere, processes ended with their tab or window, and
    terminals reopened from the session."""
    project = os.path.realpath(os.path.join(WORK, "TermKit"))
    shutil.copytree(os.path.join(samples, "ZKit"), project, ignore=shutil.ignore_patterns(".build"))
    greeter = os.path.join(project, "Sources", "ZKit", "Greeter.swift")
    env = {"ZCODE_TERMINAL_SHELL": "/bin/zsh -f"}
    z = Zcode(project, env)

    def terms(s=None):
        return (s or z.dump())["terminals"]["list"]

    def panel(s=None):
        return [t for t in terms(s) if t["location"] == "panel"]

    def index_of(t, s=None):
        return terms(s).index(t)

    def wait_line(n, predicate, timeout=30):
        try:
            return z.wait_for(lambda s: len(terms(s)) > n and any(predicate(l) for l in terms(s)[n].get("lines") or []),
                              timeout=timeout, interval=0.3)
        except TimeoutError:
            t = terms()
            raise AssertionError(f"terminal {n} of {len(t)}: {t[n]['lines'] if len(t) > n else None}") from None

    def run_in(n, command, marker):
        z.run(f"terminal type {n} {command}\\n")
        return wait_line(n, lambda l: l.strip() == marker)

    def row_of(n, predicate, s=None):
        lines = terms(s)[n]["lines"]
        return next(i for i, l in enumerate(lines) if predicate(l))

    def cell(t, row, column):
        l = t["layout"]
        return l["x"] + (column + 0.5) * l["cell_width"], l["y"] + (row + 0.5) * l["cell_height"]

    def shells():
        """zcode's children that run a shell (zcode starts others: git, swift)."""
        out = subprocess.run(["pgrep", "-P", str(z.proc.pid)], capture_output=True, text=True).stdout
        names = {int(p): subprocess.run(["ps", "-o", "comm=", "-p", p], capture_output=True, text=True).stdout.strip() for p in out.split()}
        return [pid for pid, name in names.items() if os.path.basename(name).lstrip("-") in ("zsh", "sh", "bash", "login")]

    def alive(pid):
        try:
            os.kill(pid, 0)
            return True
        except ProcessLookupError:
            return False

    def not_loaded_at_startup():
        s = z.dump()
        assert terms(s) == [] and not s["terminals"]["tab_shown"], s["terminals"]
        assert shells() == [], f"no shells at startup: {shells()}"

    def shell_in_the_project():
        z.run("keys ctrl-cmd-`", "wait 300")
        s = z.wait_for(lambda s: panel(s) and panel(s)[0]["state"] == "running", timeout=20, interval=0.3)
        t = panel(s)[0]
        assert t["shown"] and t["focused"] and s["show_debug_area"], t
        assert t["current_dir"] == project and t["process"] == "zsh", t
        assert t["pty"] == {"columns": t["layout"]["columns"], "rows": t["layout"]["rows"]}, t
        s = run_in(0, "echo $((6*7)) $TERM $TERM_PROGRAM; echo marker-1", "marker-1")
        assert "42 xterm-256color zcode" in terms(s)[0]["lines"], terms(s)[0]["lines"]
        # Typed through the real key path, Return included.
        z.run("type echo typed-$((1+1))", "keys enter")
        wait_line(0, lambda l: l == "typed-2")

    def keys_go_to_the_shell():
        # ⌃C interrupts what's running.
        z.run("terminal type 0 sleep 30\\n")
        z.wait_for(lambda s: terms(s)[0]["process"] == "sleep", timeout=10, interval=0.3)
        z.run("keys ctrl-c")
        z.wait_for(lambda s: terms(s)[0]["process"] == "zsh", timeout=10, interval=0.3)
        # ⌃\ (zcode's Next Change) and ⌃6 reach the shell as bytes 28 and 30.
        read_key = 'stty -isig; read -k1 k; stty isig; echo "<$((#k))>"'
        for keys, code in (("ctrl-\\", 28), ("ctrl-6", 30)):
            z.run(f"terminal type 0 {read_key}\\n", "wait 400", f"keys {keys}")
            # The tty echoes the control character before the shell prints it.
            wait_line(0, lambda l: l.endswith(f"<{code}>"))
        # ↑ is the shell's history.
        z.run("keys up", "wait 300")
        t = terms()[0]
        assert t["lines"][t["cursor"][0]].endswith(read_key), t["lines"]
        z.run("keys ctrl-u")
        # ⌘ keys keep zcode's meaning: ⌘K clears the terminal (Clear Console).
        run_in(0, "seq 1 100; echo marker-2", "marker-2")
        z.run("keys cmd-k", "wait 300")
        t = terms()[0]
        assert t["history"] == 0 and t["cursor"][0] == 0 and "marker-2" not in t["lines"], (t["history"], t["cursor"], t["lines"][:3])

    def selection_and_copy():
        s = run_in(0, "clear; echo select-me-12345 tail; echo marker-3", "marker-3")
        t = terms(s)[0]
        row = row_of(0, lambda l: l.startswith("select-me"), s)
        x1, y = cell(t, row, 0)
        x2, _ = cell(t, row, 14)
        z.run(f"drag {x1 - t['layout']['cell_width'] / 2 + 1} {y} {x2 + t['layout']['cell_width'] / 2 - 1} {y}", "wait 200")
        assert terms()[0]["selection"] == "select-me-12345", terms()[0]["selection"]
        z.run("keys cmd-c")
        clip = subprocess.run(["pbpaste"], capture_output=True, text=True).stdout
        assert clip == "select-me-12345", clip
        # Double-click takes a word; typing ends the selection.
        xw, _ = cell(t, row, 17)
        z.run(f"double-click {xw} {y}", "wait 200")
        assert terms()[0]["selection"] == "tail", terms()[0]["selection"]
        z.run("type x", "keys backspace", "wait 200")
        assert terms()[0]["selection"] is None
        # ⌘V pastes (the shell gets it as a bracketed paste).
        z.run("keys cmd-v", "wait 300")
        assert terms()[0]["lines"][terms()[0]["cursor"][0]].endswith("select-me-12345"), terms()[0]["lines"]
        z.run("keys ctrl-u")

    def scrollback():
        run_in(0, "seq 1 500; echo marker-4", "marker-4")
        t = terms()[0]
        assert t["history"] > 400 and t["display_offset"] == 0, (t["history"], t["display_offset"])
        x, y = cell(t, 2, 10)
        z.run(f"wheel {x} {y} 200", "wait 200")
        t = terms()[0]
        assert t["display_offset"] == 200 and "marker-4" not in t["lines"], t["display_offset"]
        z.run("terminal scroll 0 -50", "wait 200")
        assert terms()[0]["display_offset"] == 150, terms()[0]["display_offset"]
        # Typing comes back to the bottom.
        z.run("type x", "wait 200")
        assert terms()[0]["display_offset"] == 0
        z.run("keys backspace")

    def links_open_in_the_editor():
        s = run_in(0, "clear; echo 'error at Sources/ZKit/Greeter.swift:3:5: something'; echo marker-5", "marker-5")
        t = terms(s)[0]
        row = row_of(0, lambda l: l.startswith("error at"), s)
        x, y = cell(t, row, 20)
        z.run(f"cmd-click {x} {y}", "wait 500")
        a = z.dump()["active"]
        assert a["path"] == greeter and a["line"] == 3 and a["column"] == 5, (a["path"], a["line"], a["column"])
        # A link a program marks (OSC 8) over other text.
        s = run_in(0, "clear; printf '\\033]8;;file://%s\\033\\\\the greeter\\033]8;;\\033\\\\ ok\\012' \"$PWD/Sources/ZKit/Greeter.swift\"; echo marker-5b", "marker-5b")
        row = row_of(0, lambda l: l.startswith("the greeter ok"), s)
        z.run(f"open {os.path.join(project, 'Package.swift')}", "wait 300")
        assert json.loads(z.run(f"terminal link 0 {row} 4")[-1]) == {"opened": True}
        assert z.dump()["active"]["path"] == greeter, z.dump()["active"]["path"]
        z.run(f"open {os.path.join(project, 'Package.swift')}", "keys cmd-w", "wait 200")
        assert z.dump()["panes"]["list"][0]["tabs"] == ["Greeter.swift"], z.dump()["panes"]["list"][0]["tabs"]
        # A missing file isn't a link.
        s = run_in(0, "echo Sources/Gone.swift:3; echo marker-6", "marker-6")
        row = row_of(0, lambda l: l.startswith("Sources/Gone"), s)
        assert json.loads(z.run(f"terminal link 0 {row} 3")[-1]) == {"opened": False}, "a missing file isn't a link"

    def resizing():
        z.run("terminal focus 0", "wait 200")
        before = terms()[0]["pty"]
        z.run("keys cmd-+", "keys cmd-+", "wait 400")
        after = terms()[0]["pty"]
        assert after["columns"] < before["columns"], (before, after)
        s = run_in(0, "stty size; echo marker-7", "marker-7")
        assert f"{after['rows']} {after['columns']}" in terms(s)[0]["lines"], (after, terms(s)[0]["lines"])
        z.run("keys ctrl-cmd-0", "wait 400")
        assert terms()[0]["pty"] == before, (before, terms()[0]["pty"])

    def several_terminals():
        # View ▸ New Terminal: a second one in the debug area, shown.
        z.run("action zcode::NewTerminal", "wait 300")
        s = z.wait_for(lambda s: len(panel(s)) == 2 and panel(s)[1]["state"] == "running", timeout=20, interval=0.3)
        assert panel(s)[1]["shown"] and not panel(s)[0]["shown"] and s["terminals"]["panel_active"] == 1
        run_in(1, "echo second; echo marker-8", "marker-8")
        # File ▸ New ▸ Terminal Tab: one in the editor pane.
        z.run(f"open {greeter}", "wait 300", "action zcode::NewTerminalTab", "wait 300")
        s = z.wait_for(lambda s: len(terms(s)) == 3 and terms(s)[2]["state"] == "running", timeout=20, interval=0.3)
        t = terms(s)[2]
        assert t["location"] == "pane 0 tab 1" and t["focused"] and t["current_dir"] == project, t
        assert s["panes"]["list"][0]["tabs"] == ["Greeter.swift", "zsh"], s["panes"]["list"][0]["tabs"]
        run_in(2, "echo in-a-tab; echo marker-9", "marker-9")
        # The title follows what's running.
        z.run("terminal type 2 sleep 30\\n")
        z.wait_for(lambda s: s["panes"]["list"][0]["tabs"][1] == "sleep", timeout=10, interval=0.3)
        z.run("keys ctrl-c")
        # File ▸ Open Terminal at Location: in the item's folder.
        z.run(f"terminal at {greeter}", "wait 300")
        s = z.wait_for(lambda s: len(panel(s)) == 3 and panel(s)[2]["state"] == "running", timeout=20, interval=0.3)
        assert panel(s)[2]["current_dir"] == os.path.dirname(greeter), panel(s)[2]

    def flood_keeps_typing_fast():
        z.run(f"open {greeter}", "wait 300", "keys cmd-down", "keys enter")
        quiet = json.loads(z.run("type-latency abcdefghijklmnopqrstuvwxyz")[-1])
        z.run("terminal panel -- yes | head -n 3000000; echo flood-done; sleep 60", "wait 200", "pane focus 0")
        start = time.time()
        n = len(terms()) - 2  # the new panel terminal, before the pane's tab
        flood = json.loads(z.run("type-latency abcdefghijklmnopqrstuvwxyz")[-1])
        assert terms()[n]["process"] in ("yes", "head", "sh", "bash"), terms()[n]["process"]
        wait_line(n, lambda l: l == "flood-done", timeout=120)
        took = time.time() - start
        print(f"    keystroke → frame in the editor: quiet {quiet['median_ms']:.1f} ms median, "
              f"while `yes` floods a terminal {flood['median_ms']:.1f} ms (p95 {flood['p95_ms']:.1f}); 3M lines shown in {took:.1f} s")
        assert flood["median_ms"] < 16 and flood["p95_ms"] < 33, flood
        for _ in range(30):
            if not z.dump()["active"]["dirty"]:
                break
            z.run("keys cmd-z")
        z.run(f"terminal close {n}", "wait 300")

    def closing_ends_processes():
        # A pane's terminal: ⌘W closes the tab and its processes.
        tab = next(i for i, t in enumerate(terms()) if t["location"].startswith("pane "))
        s = run_in(tab, "sleep 300 & echo pid=$!; echo marker-10", "marker-10")
        pid = int(next(l for l in terms(s)[tab]["lines"] if l.startswith("pid="))[4:])
        assert alive(pid)
        z.run(f"terminal focus {tab}", "wait 200")
        assert terms()[tab]["focused"], terms()[tab]
        z.run("keys cmd-w", "wait 500")
        s = z.dump()
        assert all(not t["location"].startswith("pane ") for t in terms(s)) and s["panes"]["list"][0]["tabs"] == ["Greeter.swift"], \
            (s["panes"]["list"][0]["tabs"], [(t["location"], t["focused"], t["state"]) for t in terms(s)])
        deadline = time.time() + 5
        while alive(pid) and time.time() < deadline:
            time.sleep(0.1)
        assert not alive(pid), "the tab's background job is gone"
        # The debug area's: ⌘W while it has focus.
        last = len(panel()) - 1
        z.run(f"terminal focus {last}", "wait 200", "keys cmd-w", "wait 300")
        assert len(panel()) == last, len(panel())
        # `exit` closes the user's shell's tab.
        z.run("terminal type 1 exit\\n")
        z.wait_for(lambda s: len(panel(s)) == 1, timeout=10, interval=0.3)
        # A command's terminal stays when it ends, saying how.
        z.run("terminal panel -- echo bye; exit 3")
        s = z.wait_for(lambda s: panel(s)[-1]["state"] == "ended", timeout=10, interval=0.3)
        assert panel(s)[-1]["exit"] == 3 and "[Process exited with code 3]" in panel(s)[-1]["lines"], panel(s)[-1]
        assert panel(s)[-1]["title"].endswith("(ended)"), panel(s)[-1]["title"]
        z.run(f"terminal close {len(panel(s)) - 1}")

    def reopened_from_the_session():
        nonlocal z
        # The debug area's terminal cd'd into Sources, and a tab in Tests.
        run_in(0, "cd Sources; echo marker-11", "marker-11")
        z.run("action zcode::NewTerminalTab", "wait 300")
        tab = len(terms()) - 1
        z.wait_for(lambda s: terms(s)[tab]["state"] == "running", timeout=20, interval=0.3)
        run_in(tab, "cd ../TermKit/Tests; sleep 300 & echo pid=$!; echo marker-12", "marker-12")
        pid = int(next(l for l in terms()[tab]["lines"] if l.startswith("pid="))[4:])
        assert shells(), "shells run"
        z.quit()
        deadline = time.time() + 5
        while alive(pid) and time.time() < deadline:
            time.sleep(0.1)
        assert not alive(pid), "quitting ends the terminals' processes"
        z = Zcode(project, env)
        s = z.wait_for(lambda s: len(terms(s)) == 2, timeout=20, interval=0.3)
        p, t = terms(s)
        assert p["location"] == "panel" and p["dir"] == os.path.join(project, "Sources"), p
        assert t["location"] == "pane 0 tab 1" and t["dir"] == os.path.join(project, "Tests"), t
        # Fresh shells there, started when shown (the tab is; the debug
        # area's Terminal tab was too).
        s = z.wait_for(lambda s: all(x["state"] == "running" for x in terms(s)), timeout=20, interval=0.3)
        assert all(x["current_dir"] == x["dir"] and x["history"] == 0 for x in terms(s)), terms(s)
        assert not any("marker-12" in l for l in terms(s)[1]["lines"]), "a new shell, not the old output"
        z.run("terminal close 1", "terminal close 0", "wait 300")

    try:
        check("Terminal: nothing starts until one is opened", not_loaded_at_startup)
        check("Terminal: ⌃⌘` opens a shell in the project's folder in the debug area, at the grid's size", shell_in_the_project)
        check("Terminal: ⌃C, ⌃\\ and ⌃6 go to the shell, ↑ is its history; ⌘K clears", keys_go_to_the_shell)
        check("Terminal: drag to select, ⌘C copies, double-click a word, ⌘V pastes", selection_and_copy)
        check("Terminal: scrollback through the wheel; typing comes back to the bottom", scrollback)
        check("Terminal: ⌘-click on file:line:column or a program's OSC 8 link opens the editor there; missing files aren't links", links_open_in_the_editor)
        check("Terminal: a bigger font gives the shell fewer columns (SIGWINCH, stty size)", resizing)
        check("Terminal: several in the debug area, one in an editor tab, Open Terminal at Location", several_terminals)
        check("Terminal: `yes` flooding a terminal doesn't slow typing in an editor pane", flood_keeps_typing_fast)
        check("Terminal: ⌘W and `exit` close a terminal and end its processes; a command's says how it ended", closing_ends_processes)
        check("Terminal: quitting ends the shells; a relaunch reopens fresh ones in the same folders", reopened_from_the_session)
    finally:
        z.quit()


VIM_PLIST = """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>VimKit</string>
</dict>
</plist>
"""


def long_line_checks():
    """A minified script, one ~430 KB line, in a folder of its own: typing at
    its end and Page Down cost about what a short file's do, a frame shapes
    only what's on screen, soft wrap wraps it within the same budgets, and
    the text stays right throughout (docs/scale.md, Performance batch 2)."""
    project = os.path.realpath(os.path.join(WORK, "LongLines"))
    os.makedirs(project, exist_ok=True)
    minified = os.path.join(project, "min.js")
    short = os.path.join(project, "short.js")
    line = ";".join(f"var a{i}=function(b){{return b*{i}+Math.sqrt({i})}}" for i in range(8000))
    with open(minified, "w") as f:
        f.write(line)
    with open(short, "w") as f:
        f.write("".join(f"var x{i} = function(b) {{ return b * {i}; }};\n" for i in range(200)))
    z = Zcode(project)

    def undo_all():
        for _ in range(50):
            if not z.dump()["active"]["dirty"]:
                return
            z.run("keys cmd-z", "wait 50")
        raise AssertionError("still dirty after 50 undos")

    def pages():
        frames = [json.loads(z.run("bench keys pagedown pagedown pagedown pagedown pagedown")[-1]) for _ in range(3)]
        return statistics.median(f["work_ms"] for f in frames)

    baseline = {}

    def typing_and_page_down():
        z.run(f"open {short}:100", "wait 800", "keys end")
        baseline["typing"] = json.loads(z.run("type-latency abcdefghij")[-1])["work_median_ms"]
        undo_all()
        z.run("keys cmd-up")
        baseline["pages"] = pages()
        z.run(f"open {minified}", "wait 1500", "keys cmd-down", "wait 200")
        s = z.dump()
        assert s["active"]["column"] == len(line) + 1, s["active"]["column"]
        # The caret's end of the line is on screen, and little of it shaped.
        assert 0 < s["active"]["laid_out_bytes"] < 20_000, s["active"]["laid_out_bytes"]
        typed = json.loads(z.run("type-latency abcdefghij")[-1])
        s = z.dump()
        assert s["active"]["line_text"] == line + "abcdefghij" and s["active"]["column"] == len(line) + 11, s["active"]["column"]
        assert typed["work_median_ms"] < baseline["typing"] * 3 + 5, (typed, baseline)
        undo_all()
        z.run("keys cmd-up")
        paged = pages()
        assert paged < baseline["pages"] * 3 + 10, (paged, baseline)
        assert z.dump()["active"]["laid_out_bytes"] < 20_000
        print(f"    typing {typed['work_median_ms']:.1f} ms of work a key (short file {baseline['typing']:.1f}), five Page Downs {paged:.1f} ms ({baseline['pages']:.1f})")

    def soft_wrap():
        z.run(f"open {minified}", "wait 300")
        toggled = json.loads(z.run("bench action zcode::ToggleSoftWrap")[-1])
        assert toggled["drawn_ms"] < 2000, toggled
        z.run("wait 300", "keys cmd-down", "wait 200")
        s = z.dump()
        # Only the rows on screen and around the caret are shaped.
        assert 0 < s["active"]["laid_out_bytes"] < 60_000, s["active"]["laid_out_bytes"]
        typed = json.loads(z.run("type-latency abcdefghij")[-1])
        assert z.dump()["active"]["line_text"] == line + "abcdefghij"
        assert typed["work_median_ms"] < baseline.get("typing", 3) * 3 + 8, (typed, baseline)
        # Up a row stays in the line, a row's width to the left.
        z.run("keys up", "wait 100")
        s = z.dump()
        assert s["active"]["line"] == 1 and len(line) - 400 < s["active"]["column"] < len(line), s["active"]["column"]
        undo_all()
        z.run("action zcode::ToggleSoftWrap", "wait 200")
        print(f"    soft wrap on: {toggled['drawn_ms']:.0f} ms to the frame on screen, typing {typed['work_median_ms']:.1f} ms of work a key")

    try:
        check("Long lines: typing at the end of a minified line and Page Down cost about what a short file's do, and only what's on screen is shaped", typing_and_page_down)
        check("Long lines: soft wrap on a minified line, the rows on screen shaped, typing and moving up a row", soft_wrap)
    finally:
        z.quit()


def vim_checks(samples):
    """Editor ▸ Vim Mode on a copy of ZKit: off by default and turned on for
    every tab; normal, insert and visual mode editing a Swift file through
    the real key path; `:w`, `:q` and `:q!`; search; Escape closing
    sourcekit-lsp's completion before it leaves insert mode; `:w` in a
    multibuffer saving the files it changed; terminals, property lists and
    Open Quickly's field left alone; a mode per pane; typing latency in
    insert mode against Vim mode off; and the setting after a relaunch."""
    project = os.path.realpath(os.path.join(WORK, "VimKit"))
    shutil.copytree(os.path.join(samples, "ZKit"), project, ignore=shutil.ignore_patterns(".build"))
    greeter = os.path.join(project, "Sources", "ZKit", "Greeter.swift")
    main_swift = os.path.join(project, "Sources", "zhello", "main.swift")
    big = os.path.join(project, "Sources", "ZKit", "Big.swift")
    info = os.path.join(project, "Info.plist")
    with open(big, "w") as f:
        f.write(big_swift())
    with open(info, "w") as f:
        f.write(VIM_PLIST)
    original = open(greeter).read()
    name_line = line_of(greeter, "public let name")
    init_line = line_of(greeter, "self.name = name")
    env = {"ZCODE_TERMINAL_SHELL": "/bin/zsh -f"}
    z = Zcode(project, env)

    def vim(s=None):
        return (s or z.dump())["vim"]

    def mode(s=None):
        return vim(s)["mode"]

    def line(s=None):
        return (s or z.dump())["active"]["line_text"]

    def esc():
        """Escape until normal mode: a completion menu (sourcekit-lsp offers
        one while a word is typed) takes the first one."""
        for _ in range(3):
            z.run("keys escape", "wait 100")
            if mode() == "NORMAL":
                return
        raise AssertionError(f"still {mode()}")

    def off_by_default_then_every_tab():
        z.run(f"open {greeter}:{name_line}", f"open {main_swift}:1", "wait 300")
        v = vim()
        assert not v["enabled"] and v["editors_on"] == 0 and v["mode"] is None, v
        # Typing types.
        z.run("type j", "wait 100")
        assert line().startswith("j"), line()
        z.run("keys cmd-z", "wait 100")
        z.run("action zcode::ToggleVimMode", "wait 200")
        v = vim()
        assert v["enabled"] and v["editors_on"] == v["editors"] >= 2 and v["mode"] == "NORMAL", v
        z.run(f"open {greeter}:{name_line}", "wait 200")
        assert mode() == "NORMAL"

    def normal_insert_and_undo():
        z.run(f"open {greeter}:{name_line}", "wait 200", "type 0w", "wait 100")
        s = z.dump()
        assert s["active"]["column"] == 5 and not s["active"]["dirty"], s["active"]
        # ciw, typed text, Escape: one change, undone in one step.
        z.run("type ciw", "wait 100")
        assert mode() == "INSERT"
        z.run("type private")
        esc()
        s = z.dump()
        assert mode(s) == "NORMAL" and line(s) == "    private let name: String", line(s)
        assert s["active"]["column"] == 11 and s["active"]["dirty"], s["active"]
        z.run("type u", "wait 100")
        s = z.dump()
        assert line(s) == "    public let name: String" and not s["active"]["dirty"], s["active"]
        # . repeats the change on another word.
        z.run("type ciwopen")
        esc()
        z.run("type w.", "wait 100")
        assert line() == "    open open name: String", line()
        z.run("type uu", "wait 100")
        assert line() == "    public let name: String" and not z.dump()["active"]["dirty"]
        # Operators, counts and text objects.
        z.run("type $F:D", "wait 100")
        assert line() == "    public let name", line()
        z.run("type u0d2w", "wait 100")
        assert line() == "let name: String", line()
        z.run("type uyyjp", "wait 100")
        s = z.dump()
        assert s["active"]["line"] == name_line + 2 and line(s) == "    public let name: String", (s["active"]["line"], line(s))
        z.run("type uk", "wait 100")
        assert open(greeter).read() == original and not z.dump()["active"]["dirty"]

    def visual_and_search():
        z.run(f"open {greeter}:1", "wait 200", "type /greeting(", "wait 100")
        assert vim()["command_line"] == "/greeting(", vim()
        z.run("keys enter", "wait 200")
        s = z.dump()
        assert s["active"]["line"] == line_of(greeter, "func greeting") and mode(s) == "NORMAL", s["active"]
        # Visual line, indent, and undo.
        z.run("type Vj>", "wait 100")
        s = z.dump()
        assert line(s).startswith("        public func greeting"), line(s)
        z.run("type u", "wait 100")
        # Visual: the selection is what Vim selects, both ends included.
        z.run("type 0wve", "wait 100")
        s = z.dump()
        assert mode(s) == "VISUAL" and s["active"]["selection"] == "public", s["active"]["selection"]
        z.run("keys escape", "wait 100")
        assert mode() == "NORMAL" and not z.dump()["active"]["dirty"]

    def write_and_quit():
        z.run(f"open {greeter}:{name_line}", "wait 200", "type 0wcwinternal")
        esc()
        z.run("type :w", "keys enter", "wait 300")
        s = z.dump()
        assert "internal let name" in open(greeter).read() and not s["active"]["dirty"], s["active"]
        # :q with unsaved changes says so; :q! closes the tab without saving.
        z.run("type x:q", "keys enter", "wait 200")
        s = z.dump()
        assert s["active"]["path"] == greeter and s["active"]["dirty"], s["active"]
        assert vim(s)["message"].startswith("E37"), vim(s)
        z.run("type :q!", "keys enter", "wait 300")
        s = z.dump()
        tabs = s["panes"]["list"][0]["tabs"]
        assert "Greeter.swift" not in tabs and "main.swift" in tabs, tabs
        assert "internal let name" in open(greeter).read(), "the x wasn't saved"
        # :wq saves and closes; the window stays.
        z.run(f"open {greeter}:{name_line}", "wait 200", "type 0wcwpublic")
        esc()
        z.run("type :wq", "keys enter", "wait 300")
        s = z.dump()
        assert open(greeter).read() == original and "Greeter.swift" not in s["panes"]["list"][0]["tabs"], s["panes"]

    def escape_closes_completion_first():
        z.run(f"open {greeter}:{init_line}", "wait 300")
        z.wait_for(lambda s: s["lsp"] == "Ready", timeout=120)
        z.run("type A", "keys enter", "wait 300")
        for attempt in range(4):
            z.run("type self.na", "wait 500")
            try:
                z.wait_for(lambda s: s["active"]["completion_menu_open"], timeout=10, interval=0.3)
                break
            except TimeoutError:
                z.run("keys ctrl-u", "wait 1000")
        s = z.dump()
        assert s["active"]["completion_menu_open"] and mode(s) == "INSERT", s["active"]
        z.run("keys escape", "wait 200")
        s = z.dump()
        assert not s["active"]["completion_menu_open"] and mode(s) == "INSERT", (s["active"]["completion_menu_open"], mode(s))
        z.run("keys escape", "wait 200")
        assert mode() == "NORMAL"
        z.run("type u", "wait 200")
        assert not z.dump()["active"]["dirty"]

    def multibuffer_write():
        z.run("keys cmd-shift-f", "wait 200", "type greeting", "keys enter")
        z.wait_for(lambda s: s["find_results"] > 0, timeout=20, interval=0.3)
        z.run("action zcode::ShowFindResultsInEditor", "wait 500")
        s = z.dump()
        assert s["panes"]["list"][0]["active"] == "Find “greeting”" and mode(s) == "NORMAL", (s["panes"]["list"][0], vim(s))
        m = s["multibuffer"]["active"]
        n = next(n for n in m["numbers"] if n["file"] == "Greeter.swift" and n["line"] == line_of(greeter, "func greeting"))
        z.run(f"click {n['x'] + 300} {n['y']}", "wait 200")
        assert mode() == "NORMAL", vim()
        z.run("type A // vim")
        esc()
        m = z.dump()["multibuffer"]["active"]
        assert m["dirty"] == ["Greeter.swift"] and "// vim" in m["text"], m["dirty"]
        assert "// vim" not in open(greeter).read()
        z.run("type :w", "keys enter", "wait 300")
        m = z.dump()["multibuffer"]["active"]
        assert not m["dirty"] and "// vim" in open(greeter).read(), m["dirty"]
        z.run("type u:w", "keys enter", "wait 300")
        assert open(greeter).read() == original
        z.run("type :q", "keys enter", "wait 300")
        assert not any(t.startswith("Find") for t in z.dump()["panes"]["list"][0]["tabs"])

    def left_alone():
        # Open Quickly's field types what's typed.
        z.run("keys cmd-shift-o", "wait 300", "type Greeterjk", "keys backspace backspace", "wait 300", "keys enter", "wait 300")
        s = z.dump()
        assert s["active"]["path"] == greeter and not s["modal_open"], s["active"]["path"]
        # The terminal gets every key.
        z.run("keys ctrl-cmd-`", "wait 300")
        s = z.wait_for(lambda s: s["terminals"]["list"] and s["terminals"]["list"][0]["state"] == "running", timeout=20, interval=0.3)
        z.run("type echo vim-jk-$((1+1))", "keys enter")
        z.wait_for(lambda s: "vim-jk-2" in (s["terminals"]["list"][0].get("lines") or []), timeout=10, interval=0.3)
        s = z.dump()
        assert s["active"]["path"] == greeter and not s["active"]["dirty"], "the editor behind kept its text"
        z.run("keys cmd-shift-y", "wait 200")
        # A property list's outline has no Vim mode; as source code it has.
        z.run(f"open {info}", "wait 500")
        s = z.dump()
        assert s["active"]["plist"] and not s["active"]["plist"]["as_source"] and mode(s) is None, (s["active"]["plist"], vim(s))
        z.run(f"open-as {info} source", "wait 300")
        assert mode() == "NORMAL", vim()
        z.run(f"open-as {info} plist", "wait 300")

    def a_mode_per_pane():
        z.run(f"open {greeter}:{name_line}", "wait 200", "keys ctrl-cmd-t", "wait 500")
        panes = z.dump()["panes"]["list"]
        assert len(panes) == 2 and panes[1]["focused"], panes
        z.run("type i", "wait 100")
        panes = z.dump()["panes"]["list"]
        assert [p["shown"]["vim"] for p in panes] == ["NORMAL", "INSERT"], [p["shown"] for p in panes]
        z.run("pane focus 0", "wait 200")
        s = z.dump()
        assert mode(s) == "NORMAL" and [p["shown"]["vim"] for p in s["panes"]["list"]] == ["NORMAL", "INSERT"]
        z.run("pane focus 1")
        esc()
        z.run("keys ctrl-shift-cmd-w", "wait 300")
        assert len(z.dump()["panes"]["list"]) == 1

    def typing_latency():
        z.run(f"open {big}:3001", "wait 1500")
        z.run("vim off", "wait 200", "keys cmd-right")
        off = json.loads(z.run("type-latency abcdefghijklmnopqrstuvwxyz")[-1])
        z.run("keys cmd-z", "wait 300", "vim on", "wait 200", "type A", "wait 100")
        assert mode() == "INSERT"
        on = json.loads(z.run("type-latency abcdefghijklmnopqrstuvwxyz")[-1])
        esc()
        z.run("type u", "wait 300")
        print(f"      typing in a 6,000-line file, median / p95: Vim mode off {off['median_ms']:.1f} / {off['p95_ms']:.1f} ms, "
              f"insert mode {on['median_ms']:.1f} / {on['p95_ms']:.1f} ms")
        assert on["median_ms"] < 16 and not z.dump()["active"]["dirty"], on

    def kept_after_a_relaunch():
        nonlocal z
        z.quit()
        z = Zcode(project, env)
        z.run("vim on", f"open {greeter}:{name_line}", "wait 300")
        z.quit()
        # Zcode() turns it off for each instance; this one reads it as saved.
        z = Zcode(project, env, keep_vim=True)
        s = z.dump()
        assert vim(s)["enabled"] and mode(s) == "NORMAL", vim(s)
        z.run("type 0wv", "wait 100")
        assert mode() == "VISUAL"
        z.run("keys escape", "action zcode::ToggleVimMode", "wait 200")
        v = vim()
        assert not v["enabled"] and v["editors_on"] == 0, v

    try:
        check("Vim Mode: off by default; Editor ▸ Vim Mode turns it on in every tab", off_by_default_then_every_tab)
        check("Vim Mode: ciw, . repeat, D, d2w, yy p and u through the real key path, undone in one step each", normal_insert_and_undo)
        check("Vim Mode: / search, V j >, and a visual selection both ends included", visual_and_search)
        check("Vim Mode: :w saves, :q refuses unsaved changes (E37), :q! and :wq close the tab, not the window", write_and_quit)
        check("Vim Mode: Escape closes sourcekit-lsp's completion before it leaves insert mode", escape_closes_completion_first)
        check("Vim Mode: in a multibuffer, :w saves the files the tab changed", multibuffer_write)
        check("Vim Mode: Open Quickly's field, terminals and property list outlines are left alone", left_alone)
        check("Vim Mode: each pane's editor has its own mode", a_mode_per_pane)
        check("Vim Mode: typing latency in insert mode against Vim mode off", typing_latency)
        check("Vim Mode: kept after a relaunch, and turned off again", kept_after_a_relaunch)
    finally:
        try:
            z.run("vim off")
        except Exception:
            pass
        z.quit()


def agent_checks(samples):
    """The agent panel on a copy of ZKit, with scripts/fake_acp_agent.py as
    the agent (no network or account) in a configuration folder of its own:
    nothing loaded until ⌘0 and no agent until a message is sent; streamed
    answers and thinking; a permission prompt, then a write that lands in
    the open buffer (unsaved edits and all, as the agent read them) as one
    ⌘Z; Reject; files created and changed by the agent itself taken into
    buffers; Review against what a file was before; writes outside the
    project refused; @-mentions typed and picked, sent with the buffer's
    text; Follow; plan, modes, Stop, a crash, signing in; New Thread letting
    go of buffers; the panel and Follow after a relaunch without an agent;
    the agent ended with the window."""
    project = os.path.realpath(os.path.join(WORK, "AgentKit"))
    shutil.copytree(os.path.join(samples, "ZKit"), project, ignore=shutil.ignore_patterns(".build"))
    greeter = os.path.join(project, "Sources", "ZKit", "Greeter.swift")
    main_swift = os.path.join(project, "Sources", "zhello", "main.swift")
    package = os.path.join(project, "Package.swift")
    made = os.path.join(project, "Sources", "ZKit", "Made.swift")
    config = tempfile.mkdtemp(prefix="zcode-e2e-agent-config-")
    acp_log = os.path.join(config, "acp.jsonl")
    fake = [sys.executable, os.path.join(ROOT, "scripts", "fake_acp_agent.py")]
    with open(os.path.join(config, "settings.json"), "w") as f:
        json.dump({
            "agent_servers": {
                "fake": {"command": fake[0], "args": fake[1:], "env": {"FAKE_ACP_LOG": acp_log}},
                "fake-auth": {"command": fake[0], "args": fake[1:], "env": {"FAKE_ACP_REQUIRE_AUTH": "1"}},
            },
            "agent": {"default_agent": "fake"},
        }, f)
    env = {"ZCODE_CONFIG_DIR": config, "ZCODE_AGENT_NO_LOGIN_SHELL": "1"}
    with open(greeter) as f:
        greeter_original = f.read()
    with open(package) as f:
        package_original = f.read()
    z = Zcode(project, env)

    def agent(s=None):
        return (s or z.dump())["agent"]

    def entries(s=None):
        return agent(s)["entries"]

    def agents_running():
        """zcode's children running the fake agent."""
        out = subprocess.run(["pgrep", "-P", str(z.proc.pid)], capture_output=True, text=True).stdout
        return [int(p) for p in out.split() if "fake_acp_agent" in subprocess.run(["ps", "-o", "args=", "-p", p], capture_output=True, text=True).stdout]

    def alive(pid):
        try:
            os.kill(pid, 0)
            return True
        except ProcessLookupError:
            return False

    def turn(text, done=lambda a: a["status"] == "idle", timeout=20):
        """Sends a message and waits for the turn to end (or `done`)."""
        before = len(entries())
        z.run(f"agent send {text}")
        return z.wait_for(lambda s: len(entries(s)) > before and done(agent(s)), timeout=timeout, interval=0.2)

    def last_text(s=None):
        return entries(s)[-1].get("text", "")

    def pending(s=None):
        return [e for e in entries(s) if e["kind"] == "tool" and (e["permission"] or {}).get("pending")]

    def not_loaded_at_startup():
        s = z.dump()
        assert agent(s) == {"shown": False, "loaded": False}, agent(s)
        assert agents_running() == [], agents_running()

    def cmd_0_shows_the_panel_without_an_agent():
        z.run("keys cmd-0", "wait 300")
        a = agent()
        assert a["shown"] and a["composer_focused"] and a["agent"] == "fake", a
        assert a["agents"] == ["claude", "codex", "gemini", "fake", "fake-auth"], a["agents"]
        assert not a["running"] and a["pid"] is None and agents_running() == [], a
        # ⌘0 from the message field hides it; again shows it.
        z.run("keys cmd-0", "wait 200")
        assert not agent()["shown"]
        z.run("keys cmd-0", "wait 200")
        assert agent()["shown"] and agent()["composer_focused"]

    def streams_an_answer():
        # Typed and sent with Return, through the real key path.
        z.run("type hello", "keys enter")
        s = z.wait_for(lambda s: agent(s)["status"] == "idle" and len(entries(s)) == 3, timeout=20, interval=0.2)
        e = entries(s)
        assert [x["kind"] for x in e] == ["user", "thought", "assistant"], e
        assert e[0]["text"] == "hello" and e[1]["text"] == "The user says hello.", e
        assert e[2]["text"] == "# Hello from the fake agent\n\n- one\n- two\n", e
        a = agent(s)
        assert a["running"] and a["session"] == "fake-session-1" and a["composer"] == "" and a["stop_reason"] == "EndTurn", a
        assert agents_running() == [a["pid"]], (agents_running(), a["pid"])
        assert a["list_items"] == 3 and a["commands"] == ["init"], a
        # What zcode told the agent about itself.
        with open(acp_log) as f:
            sent = [json.loads(l)["msg"] for l in f if json.loads(l)["dir"] == "in"]
        init = next(m for m in sent if m.get("method") == "initialize")["params"]
        assert init["protocolVersion"] == 1 and init["clientCapabilities"]["fs"] == {"readTextFile": True, "writeTextFile": True}, init
        assert init["clientCapabilities"].get("terminal") is False and init["clientInfo"]["name"] == "zcode", init
        new = next(m for m in sent if m.get("method") == "session/new")["params"]
        assert new["cwd"] == project, new
        # zcode's own MCP server, for this panel (scripts/e2e.py's MCP checks use it).
        servers = new["mcpServers"]
        assert [m["name"] for m in servers] == ["zcode"] and servers[0]["args"] == ["mcp"], servers
        assert os.path.realpath(servers[0]["command"]) == os.path.realpath(BINARY), servers
        server_env = {e["name"]: e["value"] for e in servers[0]["env"]}
        assert len(server_env["ZCODE_MCP_AGENT"]) == 32 and server_env["ZCODE_MCP_SOCKET"] == os.path.join(config, "zcode.sock"), server_env

    def permission_then_an_undoable_write():
        z.run(f"open {greeter}:1", "wait 300", "type // unsaved", "keys enter")
        assert z.dump()["active"]["dirty"]
        s = turn("edit Sources/ZKit/Greeter.swift public struct => PUBLIC struct", done=lambda a: bool(pending({"agent": a})))
        call = pending(s)[0]
        assert call["title"] == "Edit Sources/ZKit/Greeter.swift" and call["permission"]["options"] == ["Allow", "Always Allow", "Reject"], call
        assert call["content"][0]["diff"] == greeter, call
        with open(greeter) as f:
            assert f.read() == greeter_original, "nothing written before the answer"
        assert agent(s)["status"] == "generating"
        z.run("agent allow")
        s = z.wait_for(lambda s: agent(s)["status"] == "idle", timeout=20, interval=0.2)
        assert last_text(s) == "Changed Sources/ZKit/Greeter.swift.", last_text(s)
        tool = [e for e in entries(s) if e["kind"] == "tool"][-1]
        assert tool["status"] == "Completed" and tool["permission"]["outcome"] == "allow", tool
        expected = "// unsaved\n" + greeter_original.replace("public struct", "PUBLIC struct", 1)
        with open(greeter) as f:
            assert f.read() == expected, "the agent read the unsaved buffer and its write was saved"
        active = s["active"]
        assert active["text"] == expected and not active["dirty"], active["dirty"]
        assert [c["path"] for c in agent(s)["changes"]] == [greeter], agent(s)["changes"]
        assert agent(s)["opened_buffers"] == [], "the file was open already"
        # One ⌘Z in the editor takes the agent's edit back, and only it.
        z.run(f"open {greeter}", "keys cmd-z", "wait 200")
        assert z.dump()["active"]["text"] == "// unsaved\n" + greeter_original
        z.run("keys cmd-shift-z", "wait 200")
        assert z.dump()["active"]["text"] == expected

    def rejected_permission():
        s = turn("edit Sources/ZKit/Greeter.swift Greeter => Welcomer", done=lambda a: bool(pending({"agent": a})))
        z.run("agent reject")
        s = z.wait_for(lambda s: agent(s)["status"] == "idle", timeout=20, interval=0.2)
        tool = [e for e in entries(s) if e["kind"] == "tool"][-1]
        assert tool["status"] == "Failed" and tool["permission"]["outcome"] == "reject", tool
        assert last_text(s) == "Not changed.", last_text(s)
        with open(greeter) as f:
            assert "Welcomer" not in f.read()

    def created_and_self_edited_files_land_in_buffers():
        s = turn("create Sources/ZKit/Made.swift let made = 1")
        with open(made) as f:
            assert f.read() == "let made = 1\n"
        change = next(c for c in agent(s)["changes"] if c["path"] == made)
        assert change["created"] and change["added"] == 1, change
        assert made in agent(s)["opened_buffers"] and "Made.swift" in s["buffers"], s["buffers"]
        # A closed file the agent changes on disk itself, reported as the changed line only.
        s = turn("self-edit Package.swift name: \"ZKit\" => name: \"ZKitRenamed\"")
        s = z.wait_for(lambda s: package in agent(s)["opened_buffers"], timeout=10, interval=0.2)
        change = next(c for c in agent(s)["changes"] if c["path"] == package)
        assert not change["created"] and (change["added"], change["removed"]) == (1, 1), change
        z.run(f"open {package}", "wait 300")
        active = z.dump()["active"]
        assert "ZKitRenamed" in active["text"] and not active["dirty"] and active["can_undo"], active
        z.run("keys cmd-z", "wait 200")
        assert z.dump()["active"]["text"] == package_original, "⌘Z goes back to what it was before the agent"
        z.run("keys cmd-shift-z", "wait 200")

    def review_and_reject():
        z.run(f"agent review Package.swift", "wait 400")
        s = z.dump()
        tabs = s["panes"]["list"][0]
        assert tabs["active"] == "Package.swift (Agent Changes)", tabs
        diff = s["git"]["diff"]
        assert diff["base"] == "Before the Agent" and len(diff["hunks"]) == 1 and diff["base_text"] == package_original, diff
        z.run("keys cmd-w", "agent reject-file Package.swift", "wait 400")
        with open(package) as f:
            assert f.read() == package_original
        assert package not in [c["path"] for c in agent()["changes"]]
        # A file the agent created goes to the Trash.
        z.run("agent reject-file Sources/ZKit/Made.swift", "wait 400")
        assert not os.path.exists(made)
        empty_trash_item("Made.swift")

    def writes_outside_the_project_are_refused():
        s = turn("outside")
        outside = os.path.join(os.path.dirname(project), "zcode-agent-outside.txt")
        assert last_text(s).startswith(f"refused: {outside} is outside the project"), last_text(s)
        assert not os.path.exists(outside)

    def mentions_typed_and_picked():
        z.run(f"open {greeter}:1", "wait 300", "type // draft", "keys enter", "keys cmd-0", "wait 200")
        if not agent()["composer_focused"]:
            z.run("keys cmd-0", "wait 200")
        z.run("agent type echo", "keys space", "type @", "wait 400")
        s = z.dump()
        assert s["modal_open"], "typing @ opens the mention picker"
        z.run("type Greeter", "wait 300", "keys enter", "wait 300")
        a = agent()
        assert not z.dump()["modal_open"] and a["composer_focused"], a
        assert a["composer_tokens"] == [f"file://{greeter}"] and a["composer"] == "echo @Greeter.swift ", a["composer"]
        z.run(f"agent mention Sources/zhello/main.swift:1-2")
        s = turn("")
        blocks = json.loads(last_text(s).split("\n", 1)[1].rsplit("```", 1)[0])
        assert [b["type"] for b in blocks][:3] == ["text", "resource", "text"] or [b["type"] for b in blocks][:2] == ["text", "resource"], blocks
        resources = [b for b in blocks if b["type"] == "resource"]
        assert resources[0]["uri"] == f"file://{greeter}" and resources[0]["text"].startswith("// draft\n"), resources[0]
        with open(greeter) as f:
            assert not f.read().startswith("// draft"), "the buffer's text, unsaved edits included"
        with open(main_swift) as f:
            first_two = "".join(f.readlines()[:2])
        assert resources[1]["uri"] == f"file://{main_swift}#L1:2" and resources[1]["text"] == first_two, resources[1]
        assert entries(s)[-2]["mentions"] == ["@Greeter.swift", "@main.swift:1-2"], entries(s)[-2]

    def follow_the_agent():
        z.run("agent follow on", "keys cmd-0", "wait 100")
        if not agent()["composer_focused"]:
            z.run("keys cmd-0", "wait 100")
        z.run("agent type read Sources/zhello/main.swift", "keys enter")
        s = z.wait_for(lambda s: agent(s)["status"] == "idle" and s["active"] and s["active"]["path"] == main_swift, timeout=20, interval=0.2)
        assert agent(s)["composer_focused"], "the keyboard stays in the message field"
        assert last_text(s).startswith("First line: `"), last_text(s)
        z.run("agent follow off")

    def plan_modes_and_stop():
        s = turn("plan")
        assert agent(s)["plan"] == ["Completed Read the code", "InProgress Change it", "Pending Test it"], agent(s)["plan"]
        z.run("agent mode plan", "wait 300")
        s = turn("mode")
        assert last_text(s) == "mode: plan" and agent(s)["modes"]["current"] == "plan", agent(s)["modes"]
        s = turn("slow", done=lambda a: a["status"] == "generating" and "chunk 3" in a["entries"][-1].get("text", ""))
        z.run("agent cancel")
        s = z.wait_for(lambda s: agent(s)["status"] == "idle", timeout=10, interval=0.2)
        assert agent(s)["stop_reason"] == "Cancelled" and "chunk 99" not in last_text(s), agent(s)["stop_reason"]
        # Stop answers a waiting permission request as cancelled.
        turn("edit Sources/ZKit/Greeter.swift Greeter => Hi", done=lambda a: bool(pending({"agent": a})))
        z.run("agent cancel")
        s = z.wait_for(lambda s: agent(s)["status"] == "idle", timeout=10, interval=0.2)
        tool = [e for e in entries(s) if e["kind"] == "tool"][-1]
        assert tool["permission"]["outcome"] is None and not tool["permission"]["pending"] and tool["status"] == "Failed", tool

    def a_crash_then_a_new_agent():
        pid = agent()["pid"]
        s = turn("crash", done=lambda a: a["status"].startswith("error"))
        assert agent(s)["status"] == "error: fake exited: fake agent: something went wrong", agent(s)["status"]
        assert not agent(s)["running"] and not alive(pid)
        s = turn("hello")
        assert agent(s)["running"] and agent(s)["pid"] != pid and last_text(s).startswith("# Hello"), agent(s)

    def signing_in():
        z.run("agent agent fake-auth", "wait 200")
        assert entries() == [] and agent()["agent"] == "fake-auth"
        s = turn("hello", done=lambda a: a["status"].startswith("auth"))
        assert agent(s)["status"] == "auth: fake-login", agent(s)["status"]
        z.run("agent auth fake-login")
        s = z.wait_for(lambda s: agent(s)["status"] == "idle" and len(entries(s)) == 3, timeout=20, interval=0.2)
        assert last_text(s).startswith("# Hello"), "the message waiting for the sign-in is sent"
        z.run("agent agent fake", "wait 200")

    def new_thread_lets_go_of_buffers():
        turn("create Sources/ZKit/Made2.swift let two = 2")
        made2 = os.path.join(project, "Sources", "ZKit", "Made2.swift")
        assert "Made2.swift" in z.dump()["buffers"]
        z.run("keys ctrl-cmd-h", "wait 300")
        s = z.dump()
        assert entries(s) == [] and agent(s)["changes"] == [] and agent(s)["opened_buffers"] == [], agent(s)
        assert "Made2.swift" not in s["buffers"], s["buffers"]
        assert agent(s)["running"] and agent(s)["session"] is None, "the agent stays; the next message starts a session"
        s = turn("hello")
        assert agent(s)["session"] == "fake-session-2", agent(s)["session"]
        os.remove(made2)

    def relaunch_without_an_agent():
        nonlocal z
        z.run("agent follow on", "wait 200")
        pid = agent()["pid"]
        z.quit()
        deadline = time.time() + 5
        while alive(pid) and time.time() < deadline:
            time.sleep(0.1)
        assert not alive(pid), "the agent ends with zcode"
        z = Zcode(project, env)
        s = z.wait_for(lambda s: agent(s).get("shown"), timeout=10, interval=0.3)
        a = agent(s)
        assert a["follow"] and a["agent"] == "fake" and entries(s) == [], a
        assert not a["running"] and agents_running() == [], "no agent until a message is sent"

    try:
        check("Agent panel: nothing loaded at startup", not_loaded_at_startup)
        check("Agent panel: ⌘0 shows it (and hides it from its field) without starting an agent", cmd_0_shows_the_panel_without_an_agent)
        check("Agent panel: a message sent with Return starts the agent; thinking and an answer stream in", streams_an_answer)
        check("Agent panel: a permission prompt, then the agent's write in the open buffer (unsaved edits included), saved, one ⌘Z", permission_then_an_undoable_write)
        check("Agent panel: Reject leaves the file alone", rejected_permission)
        check("Agent panel: files the agent creates, or changes on disk itself, land in buffers ⌘Z undoes", created_and_self_edited_files_land_in_buffers)
        check("Agent panel: Review compares with the file before the agent; Reject puts it back (or trashes a new file)", review_and_reject)
        check("Agent panel: writes outside the project are refused", writes_outside_the_project_are_refused)
        check("Agent panel: @ opens the mention picker; mentions send the buffer's text (and a symbol's lines)", mentions_typed_and_picked)
        check("Agent panel: Follow Agent opens what it reads, keeping the keyboard in the panel", follow_the_agent)
        check("Agent panel: plan, modes, and Stop (a waiting permission request cancelled)", plan_modes_and_stop)
        check("Agent panel: a crashed agent says why; the next message starts another", a_crash_then_a_new_agent)
        check("Agent panel: an agent that wants signing in, then the waiting message", signing_in)
        check("Agent panel: New Conversation (⌃⌘H) lets go of the thread's buffers; the agent keeps running", new_thread_lets_go_of_buffers)
        check("Agent panel: the agent ends with zcode; a relaunch shows the panel and Follow, without an agent", relaunch_without_an_agent)
    finally:
        z.quit()
        shutil.rmtree(config, ignore_errors=True)


MCP_TOOLS = ["XcodeListSchemes", "XcodeSwitchScheme", "XcodeListRunDestinations", "XcodeSwitchRunDestination", "BuildProject",
             "RunProject", "StopProject", "RunAllTests", "RunSomeTests", "GetTestList", "XcodeListNavigatorIssues", "GetBuildLog",
             "GetConsoleOutput", "XcodeGetCurrentFile", "OpenFile", "EvaluateInDebugger", "TakeScreenshot", "ProfileProject",
             "ArchiveProject"]
MCP_READ_ONLY = {"XcodeListSchemes", "XcodeListRunDestinations", "GetTestList", "XcodeListNavigatorIssues", "GetBuildLog",
                 "GetConsoleOutput", "XcodeGetCurrentFile", "TakeScreenshot"}


def prediction_checks(samples):
    """Predictive code completion on a copy of ZKit, in a configuration
    folder of its own: off by default (nothing runs); then the Claude API
    provider against scripts/fake_anthropic.py (no key or network): one
    debounced, streamed request carrying the file around the cursor and the
    recent edits; ghost text drawn as an inlay and a block; Tab as one undo
    step; typing through a prediction; a moved caret cancelling a slow
    request; Escape and ⌥]; Vim's normal mode, read-only and excluded files
    left alone; errors in the status bar; ⌃⌥Space with automatic
    predictions off; typing latency with predictions on and off; the key in
    no log; then the language server provider through scripts/lsp_proxy.py
    answering textDocument/inlineCompletion."""
    project = os.path.realpath(os.path.join(WORK, "PredictKit"))
    shutil.copytree(os.path.join(samples, "ZKit"), project, ignore=shutil.ignore_patterns(".build"))
    greeter = os.path.join(project, "Sources", "ZKit", "Greeter.swift")
    with open(greeter) as f:
        greeter_original = f.read()
    config = tempfile.mkdtemp(prefix="zcode-e2e-predict-config-")
    server_dir = tempfile.mkdtemp(prefix="zcode-e2e-anthropic-")
    port_file, requests_log, reply_file = (os.path.join(server_dir, n) for n in ("port", "requests.jsonl", "reply.json"))
    stderr_log = os.path.join(server_dir, "zcode-stderr.log")
    key = "sk-ant-e2e-fake-0123456789"
    server = subprocess.Popen([sys.executable, os.path.join(ROOT, "scripts", "fake_anthropic.py"), "--port-file", port_file,
                               "--log", requests_log, "--reply", reply_file, "--key", key],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.time() + 15
    while not os.path.exists(port_file) and time.time() < deadline:
        time.sleep(0.1)
    with open(port_file) as f:
        port = int(f.read())

    def set_reply(**script):
        with open(reply_file + ".tmp", "w") as f:
            json.dump(script, f)
        os.rename(reply_file + ".tmp", reply_file)

    def requests():
        try:
            with open(requests_log) as f:
                return [json.loads(l) for l in f]
        except FileNotFoundError:
            return []

    set_reply(text="name.uppercased()", chunk=4, delay_ms=20)
    # No language server for the Claude API's checks: its completion menu,
    # which hides a prediction while it's open, would race with them.
    env = {"ZCODE_CONFIG_DIR": config, "ZCODE_AGENT_NO_LOGIN_SHELL": "1", "ANTHROPIC_API_KEY": key,
           "ZCODE_SOURCEKIT_LSP": "/usr/bin/false"}
    previous_stderr = os.environ.get("ZCODE_E2E_STDERR")
    os.environ["ZCODE_E2E_STDERR"] = stderr_log
    try:
        z = Zcode(project, env)
    except Exception:
        server.kill()
        raise
    finally:
        if previous_stderr is None:
            del os.environ["ZCODE_E2E_STDERR"]
        else:
            os.environ["ZCODE_E2E_STDERR"] = previous_stderr
    init_line = line_of(greeter, "self.name = name")

    def p(s=None):
        return (s or z.dump())["predictions"]

    def active_text():
        return z.dump()["active"]["text"]

    def new_line_after_init(text):
        """A fresh line in init (`        ` indented), `text` typed on it."""
        z.run(f"open {greeter}:{init_line}", "wait 300", "keys end", "keys enter", f"type {text}")

    def revert():
        z.run(f"open {greeter}:1", "wait 200")
        for _ in range(40):
            if not z.dump()["active"]["dirty"]:
                break
            z.run("keys escape", "keys cmd-z")
        assert active_text() == greeter_original

    def wait_p(predicate, timeout=10):
        """Waits for the predictions' state to satisfy `predicate`."""
        deadline = time.time() + timeout
        while True:
            s = z.dump()
            if predicate(p(s)):
                return s
            if time.time() > deadline:
                raise TimeoutError("predictions: " + json.dumps(p(s))[:900])
            time.sleep(0.2)

    def shown(timeout=10):
        return wait_p(lambda q: q["shown"] is not None, timeout)

    def off_by_default():
        new_line_after_init("let shout =")
        z.run("wait 800", "keys ctrl-alt-space", "wait 500")
        s = p()
        assert s["provider"] == "none" and s["requests"] == 0 and s["following"] == 0 and s["indicator"] is None, s
        assert s["ghost"] == {"inlays": [], "blocks": [], "struck": []}, s
        assert requests() == [], "nothing sent"
        revert()

    def streamed_prediction_and_tab():
        revert()
        z.run("settings set edit_predictions.provider \"claude\"",
              f"settings set edit_predictions.claude.api_url \"http://127.0.0.1:{port}\"",
              "settings set edit_predictions.debounce_ms 200", "wait 300")
        assert p()["provider"] == "claude" and p()["indicator"] == "Predictions", p()
        # Streamed slowly enough to watch it grow.
        set_reply(text=" name.uppercased()", chunk=3, delay_ms=200)
        new_line_after_init("let shout =")
        cursor = greeter_original.index("self.name = name") + len("self.name = name") + len("\n        let shout =")
        seen = []
        deadline = time.time() + 15
        while time.time() < deadline:
            s = p()
            if s["shown"] and s["shown"]["text"] not in seen:
                seen.append(s["shown"]["text"])
            if seen and seen[-1] == " name.uppercased()" and s["status"] == "idle":
                break
            time.sleep(0.1)
        assert seen and seen[-1] == " name.uppercased()" and len(seen) >= 2, f"streamed in pieces: {seen}"
        s = p()
        assert s["shown"] == {"start": cursor, "end": cursor, "text": " name.uppercased()"}, s["shown"]
        assert s["ghost"]["inlays"] == [[cursor, " name.uppercased()"]] and s["ghost"]["blocks"] == [], s["ghost"]
        assert s["indicator"] == "Prediction: ⇥ to accept", s
        assert "name.uppercased()" not in active_text(), "ghost text isn't in the buffer"
        z.run("screenshot " + os.path.join(WORK, "edit-prediction.png"))
        # One request for the whole burst of typing, with what it needs.
        sent = requests()
        assert len(sent) == 1 and s["requests"] == 1, [r["body"]["messages"][0]["content"][-80:] for r in sent]
        body, = (r["body"] for r in sent)
        assert sent[0]["key_ok"] and sent[0]["version"] == "2023-06-01" and not sent[0]["hung_up"], sent[0]
        assert body["model"] == "claude-haiku-5-5" and body["stream"] and body["stop_sequences"] == ["</insert>"], body
        assert body["output_config"] == {"effort": "low"} and body["max_tokens"] == 256, body
        prompt = body["messages"][0]["content"]
        assert '<file path="Sources/ZKit/Greeter.swift" language="swift" first_line="1">' in prompt, prompt
        assert "self.name = name\n        let shout =<|cursor|>\n    }\n" in prompt, prompt
        assert "The user's latest edits" in prompt and "+        let shout =" in prompt, prompt
        # Tab types it, as one undo step.
        z.run("keys tab", "wait 200")
        text = active_text()
        assert "        let shout = name.uppercased()\n    }" in text, text
        a = z.dump()["active"]
        assert a["column"] == len("        let shout = name.uppercased()") + 1, a["column"]
        q = p()
        assert q["accepted"] == 1 and q["ghost"]["inlays"] in ([], [[cursor + 18, q["shown"] and q["shown"]["text"]]]), q
        z.run("keys cmd-z", "wait 200")
        assert "        let shout =\n    }" in active_text(), active_text()
        revert()

    def multi_line_typing_through_and_dismissing():
        revert()
        set_reply(text=" name.uppercased()\n        print(shout)", chunk=8, delay_ms=10)
        new_line_after_init("let shout =")
        shown()
        s = wait_p(lambda q: q["status"] == "idle" and q["ghost"]["blocks"])
        row = init_line  # the new line's 0-based row
        assert p(s)["ghost"]["inlays"][0][1] == " name.uppercased()" and p(s)["ghost"]["blocks"] == [[row, ["        print(shout)"]]], p(s)["ghost"]
        before = p(s)["requests"]
        # Typing what it predicts keeps the rest, without asking again.
        z.run("keys space", "type name.", "wait 600")
        s = p()
        assert s["shown"]["text"] == "uppercased()\n        print(shout)" and s["requests"] == before, s
        # ⌥] dismisses it; typing on asks again.
        z.run("keys alt-]", "wait 200")
        assert p()["shown"] is None and p()["ghost"]["blocks"] == []
        assert "‘" not in active_text(), "⌥] didn't type a quote"
        set_reply(text="ount", chunk=8, delay_ms=10)
        z.run("type c")
        shown()
        assert p()["requests"] == before + 1
        # Escape dismisses it too (and leaves the text alone).
        text = active_text()
        z.run("keys escape", "wait 200")
        assert p()["shown"] is None and active_text() == text
        revert()

    def stale_requests_cancelled():
        revert()
        set_reply(text="name", chunk=8, delay_ms=10, hold_ms=2500)
        new_line_after_init("let a =")
        wait_p(lambda q: q["status"] == "requesting")
        before = p()["cancelled"]
        z.run("keys left", "wait 3500")
        s = p()
        assert s["cancelled"] == before + 1 and s["shown"] is None and not s["pending"], s
        assert s["status"] == "idle", s
        hung_up = [r for r in requests() if r.get("hung_up")]
        assert hung_up, "the connection was closed"
        set_reply(text="name", chunk=8, delay_ms=10)
        revert()

    def left_alone():
        revert()
        before = p()["requests"]
        # Vim's normal mode asks nothing; insert mode does.
        z.run("vim on", f"open {greeter}:{init_line}", "wait 300", "keys ctrl-alt-space", "wait 600")
        assert p()["requests"] == before and p()["shown"] is None
        set_reply(text=" // set", chunk=8, delay_ms=10)
        z.run("type A", "type x")
        shown()
        assert p()["requests"] == before + 1
        z.run("keys escape", "wait 200")
        assert p()["shown"] is None and z.dump()["vim"]["mode"] == "INSERT", "Escape closes the prediction first"
        z.run("keys escape", "vim off", "wait 200")
        revert()
        # A read-only file (a package checkout) and a secret one ask nothing.
        checkout = os.path.join(project, "Vendor", "SourcePackages", "checkouts", "Tally", "Tally.swift")
        os.makedirs(os.path.dirname(checkout))
        with open(checkout, "w") as f:
            f.write("public let tally = 1\n")
        before = p()["requests"]
        z.run(f"open {checkout}:1", "wait 300", "keys end", "keys ctrl-alt-space", "wait 600")
        assert p()["requests"] == before and z.dump()["active"]["read_only"], p()
        secret = os.path.join(project, ".env")
        with open(secret, "w") as f:
            f.write("TOKEN=abc\n")
        z.run(f"open {secret}:1", "wait 300", "keys end", "type d", "wait 800")
        s = p()
        assert s["requests"] == before and s["status"] == "excluded" and s["indicator"] == "Predictions: not for this file", s
        z.run("keys cmd-z", "keys cmd-w", "wait 200", "keys cmd-w", "wait 200")

    def errors_in_the_status_bar():
        revert()
        set_reply(status=529, error_type="overloaded_error", message="Overloaded")
        new_line_after_init("let a =")
        s = wait_p(lambda q: q["status"].startswith("failed"))
        assert p(s)["status"] == "failed: overloaded_error: Overloaded (529)" and p(s)["indicator"] == "Predictions: overloaded_error: Overloaded (529)", p(s)
        set_reply(text="name", stream_error=True, chunk=8, delay_ms=10)
        z.run("type b")
        s = wait_p(lambda q: q["status"] == "failed: overloaded_error: Overloaded")
        assert p(s)["shown"] is None, "a broken answer is dropped"
        # A refused key says so, and isn't repeated anywhere.
        z.run("settings set edit_predictions.claude.api_key \"sk-ant-wrong-e2e-key\"", "wait 200", "type c")
        wait_p(lambda q: q["status"] == "failed: the API key was refused (401)")
        assert [r["key_ok"] for r in requests()][-1] is False
        z.run("settings set edit_predictions.claude.api_key null", "wait 200")
        set_reply(text="name", chunk=8, delay_ms=10)
        z.run("type d")
        shown()
        revert()

    def asked_for_with_automatic_off():
        revert()
        z.run("settings set edit_predictions.automatic false", "wait 300")
        assert p()["indicator"] == "Predictions: ⌃⌥Space"
        before = p()["requests"]
        set_reply(text=" name.count", chunk=8, delay_ms=10)
        new_line_after_init("let n =")
        z.run("wait 800")
        assert p()["requests"] == before and p()["shown"] is None, "nothing while typing"
        # Editor ▸ Show Predictive Completion (⌃⌥Space, as in Xcode).
        z.run("keys ctrl-alt-space")
        shown()
        assert p()["shown"]["text"] == " name.count" and p()["last_request"]["invoked"], p()
        z.run("keys tab", "wait 200")
        assert "        let n = name.count\n" in active_text(), active_text()
        z.run("settings set edit_predictions.automatic true", "wait 300")
        revert()

    def typing_latency():
        revert()
        set_reply(text="name", chunk=8, delay_ms=10)
        z.run(f"open {greeter}:{init_line}", "wait 300", "keys end", "keys enter")
        on = json.loads(z.run("type-latency let measured = name.count + 1")[-1])
        revert()
        z.run("settings set edit_predictions.provider \"none\"", "wait 300")
        assert p()["following"] == 0 and p()["indicator"] is None
        z.run(f"open {greeter}:{init_line}", "wait 300", "keys end", "keys enter")
        off = json.loads(z.run("type-latency let measured = name.count + 1")[-1])
        revert()
        z.run("settings set edit_predictions.provider \"claude\"", "wait 300")
        print(f"    keystroke → frame, median / p95: predictions on {on['median_ms']:.1f} / {on['p95_ms']:.1f} ms, "
              f"off {off['median_ms']:.1f} / {off['p95_ms']:.1f} ms")
        assert on["median_ms"] < 16 and on["median_ms"] <= max(off["median_ms"] * 1.5, off["median_ms"] + 2), (on, off)

    def key_in_no_log():
        z.quit()
        with open(stderr_log) as f:
            logged = f.read()
        for secret in (key, "sk-ant-wrong-e2e-key"):
            assert secret not in logged, "a key in zcode's log"
        # The control channel's log echoes the commands it ran (the wrong
        # key was set with one), but no answer of zcode's has a key.
        with open(os.path.join(DEBUG_DIR, "log"), errors="replace") as f:
            answers = [l for l in f if not l.startswith(("ok settings set", "err settings set"))]
        assert not any(key in l or "sk-ant-wrong-e2e-key" in l for l in answers), "a key in the control channel's answers"
        assert "edit prediction: overloaded_error: Overloaded (529)" in logged, logged[-2000:]

    try:
        check("Predictions: off by default; nothing asked, followed or shown", off_by_default)
        check("Predictions (Claude API, fake server): one debounced streamed request with the file and recent edits; ghost text as an inlay; Tab as one undo step", streamed_prediction_and_tab)
        check("Predictions: several lines as an inlay and a block; typing what it predicts keeps it; ⌥] and Escape dismiss", multi_line_typing_through_and_dismissing)
        check("Predictions: a caret moved during a slow request cancels it and closes the connection", stale_requests_cancelled)
        check("Predictions: not in Vim's normal mode, read-only files or files in disabled_globs", left_alone)
        check("Predictions: API errors and a refused key in the status bar", errors_in_the_status_bar)
        check("Predictions: Show Predictive Completion (⌃⌥Space) with automatic predictions off", asked_for_with_automatic_off)
        check("Predictions: typing latency with predictions on stays level with off", typing_latency)
        check("Predictions: the API key in no log", key_in_no_log)
    finally:
        z.quit()

    traffic = os.path.join(server_dir, "lsp-traffic.log")

    def language_server_provider():
        with open(os.path.join(config, "settings.json"), "w") as f:
            json.dump({"edit_predictions": {"provider": "lsp", "debounce_ms": 200}}, f)
        z = Zcode(project, dict(env, ZCODE_SOURCEKIT_LSP=os.path.join(ROOT, "scripts", "lsp_proxy.py"),
                                ZCODE_LSP_PROXY_LOG=traffic, ZCODE_LSP_PROXY_INLINE_TEXT="name.count"))
        claude_requests = len(requests())
        try:
            z.run(f"open {greeter}:{init_line}")
            s = z.wait_for(lambda s: s["lsp"] == "Ready", timeout=120)
            assert s["language_features"]["server"]["inline_completion"], s["language_features"]["server"]
            z.run("keys end", "keys enter", "type let n =", "keys space")
            s = z.wait_for(lambda s: s["predictions"]["shown"] is not None, timeout=20, interval=0.3)
            assert s["predictions"]["shown"]["text"] == "name.count" and s["predictions"]["last_request"]["provider"] == "lsp", s["predictions"]
            assert s["predictions"]["ghost"]["inlays"][0][1] == "name.count"
            sent = open(traffic).read().splitlines()
            assert 1 <= sent.count("textDocument/inlineCompletion") <= 2, sent.count("textDocument/inlineCompletion")
            z.run("keys tab", "wait 200")
            assert "        let n = name.count\n" in z.dump()["active"]["text"], z.dump()["active"]["text"]
            assert len(requests()) == claude_requests, "nothing to the Claude API"
            for _ in range(20):
                if not z.dump()["active"]["dirty"]:
                    break
                z.run("keys cmd-z")
        finally:
            z.quit()

    try:
        check("Predictions (language server): textDocument/inlineCompletion where the server offers it", language_server_provider)
    finally:
        server.kill()
        server.wait()
        shutil.rmtree(server_dir, ignore_errors=True)
        shutil.rmtree(config, ignore_errors=True)


def mcp_checks(samples):
    """zcode's MCP server on a copy of ZKit, through `zcode mcp` and
    scripts/mcp_client.py, in a configuration folder of its own: no zcode
    running; a private socket that a silent client doesn't block; the
    handshake revisions and 2026-07-28 (server/discover, the revision per
    request, an unsupported one refused); the tools; schemes and
    destinations; a test run cancelled while it builds; a build with an
    error (progress, structured errors, issues, the log), then without;
    tests with structured results, one test, a failing one; run and the
    console; the editor; the debugger paused at a breakpoint; a window
    screenshot; and the agent panel's agent (scripts/fake_acp_agent.py)
    calling the tools, asked first for the ones that change things."""
    from mcp_client import McpClient, McpError, MODERN
    project = os.path.realpath(os.path.join(WORK, "McpKit"))
    shutil.copytree(os.path.join(samples, "ZKit"), project, ignore=shutil.ignore_patterns(".build"))
    greeter = os.path.join(project, "Sources", "ZKit", "Greeter.swift")
    main_swift = os.path.join(project, "Sources", "zhello", "main.swift")
    config = tempfile.mkdtemp(prefix="zcode-e2e-mcp-config-")
    acp_log = os.path.join(config, "acp.jsonl")
    fake = [sys.executable, os.path.join(ROOT, "scripts", "fake_acp_agent.py")]
    with open(os.path.join(config, "settings.json"), "w") as f:
        json.dump({"agent_servers": {"fake": {"command": fake[0], "args": fake[1:], "env": {"FAKE_ACP_LOG": acp_log}}},
                   "agent": {"default_agent": "fake"}}, f)
    sock = os.path.join(config, "zcode.sock")
    client_env = dict(os.environ, ZCODE_CONFIG_DIR=config)
    clients = []

    def client(modern=False, initialize=True):
        c = McpClient([BINARY, "mcp"], env=client_env, cwd=project, modern=modern)
        clients.append(c)
        if initialize and not modern:
            c.initialize()
        return c

    def text(result):
        return next((b["text"] for b in result["content"] if b["type"] == "text"), "")

    def no_zcode_running():
        empty = tempfile.mkdtemp(prefix="zcode-e2e-mcp-none-")
        out = subprocess.run([BINARY, "mcp"], env=dict(os.environ, ZCODE_CONFIG_DIR=empty), input="", capture_output=True,
                             text=True, timeout=10)
        assert out.returncode == 1 and "isn’t running" in out.stderr and out.stdout == "", out
        shutil.rmtree(empty, ignore_errors=True)

    check("MCP: `zcode mcp` with no zcode running says so and exits", no_zcode_running)
    xctrace = fake_xctrace("mcp")
    z = Zcode(project, {"ZCODE_CONFIG_DIR": config, "ZCODE_AGENT_NO_LOGIN_SHELL": "1", **xctrace.env})
    # The client most checks share.
    c = client()

    def private_socket():
        st = os.stat(sock)
        assert stat.S_ISSOCK(st.st_mode) and stat.S_IMODE(st.st_mode) == 0o600 and st.st_uid == os.getuid(), oct(st.st_mode)
        # A connection that never says anything doesn't hold up the next.
        idle = socket.socket(socket.AF_UNIX)
        idle.connect(sock)
        try:
            started = time.time()
            client()
            assert time.time() - started < 6, time.time() - started
        finally:
            idle.close()

    def handshake_revisions():
        h = client(initialize=False)
        r = h.initialize("2025-06-18")
        assert r["protocolVersion"] == "2025-06-18" and r["serverInfo"]["name"] == "zcode", r
        assert r["capabilities"] == {"tools": {"listChanged": False}} and "instructions" in r, r
        assert client(initialize=False).initialize("2099-01-01")["protocolVersion"] == "2025-11-25"
        tools = h.tools()
        assert [t["name"] for t in tools] == MCP_TOOLS, [t["name"] for t in tools]
        assert {t["name"] for t in tools if t["annotations"]["readOnlyHint"]} == MCP_READ_ONLY
        assert all(t["inputSchema"]["type"] == "object" for t in tools)
        for method, params, code in [("no/such/method", {}, -32601), ("tools/call", {"name": "no_such_tool"}, -32602),
                                     ("tools/call", {"name": "XcodeListNavigatorIssues", "arguments": {"severity": "fatal"}}, -32602),
                                     ("tools/call", {"name": "RunSomeTests", "arguments": {}}, -32602)]:
            try:
                h.request(method, params)
                raise AssertionError(f"{method} should fail")
            except McpError as e:
                assert e.code == code, (method, e.code)

    def modern_revision():
        m = client(modern=True)
        d = m.request("server/discover")
        assert d["supportedVersions"][0] == MODERN and "2025-11-25" in d["supportedVersions"], d
        assert d["resultType"] == "complete" and d["_meta"]["io.modelcontextprotocol/serverInfo"]["name"] == "zcode", d
        listed = m.request("tools/list")
        assert listed["resultType"] == "complete" and listed["ttlMs"] > 0 and len(listed["tools"]) == len(MCP_TOOLS), listed.keys()
        r = m.call("XcodeListSchemes")
        assert r["resultType"] == "complete" and r["structuredContent"]["scheme"] == "zhello", r
        try:
            c.request("tools/list", {"_meta": {"io.modelcontextprotocol/protocolVersion": "1900-01-01"}})
            raise AssertionError("an unknown revision should be refused")
        except McpError as e:
            assert e.code == -32022 and e.data["requested"] == "1900-01-01" and MODERN in e.data["supported"], e.data

    def schemes_and_destinations():
        s = c.call("XcodeListSchemes")["structuredContent"]
        assert s["project"] == {"root": project, "name": "McpKit", "kind": "swift_package"}, s["project"]
        assert "zhello" in s["schemes"] and s["scheme"] == "zhello" and s["configuration"] == "Debug", s
        d = c.call("XcodeListRunDestinations")["structuredContent"]
        assert d["destination"] == "mac" and [x["id"] for x in d["destinations"]] == ["mac"], d
        # Switched as the toolbar's menus do.
        assert c.call("XcodeSwitchScheme", {"scheme": "ZKit"})["structuredContent"]["scheme"] == "ZKit"
        assert z.dump()["scheme"] == "ZKit"
        assert c.call("XcodeSwitchScheme", {"scheme": "zhello"})["structuredContent"]["scheme"] == "zhello"
        assert c.call("XcodeSwitchRunDestination", {"destination": "My Mac"})["structuredContent"]["destination"] == "mac"
        r = c.call("BuildProject", {"scheme": "NoSuchScheme"})
        assert r["isError"] and "No scheme named" in text(r), r
        r = c.call("XcodeSwitchRunDestination", {"destination": "Nowhere"})
        assert r["isError"] and "No run destination" in text(r), r
        r = c.call("XcodeListSchemes", {"project": "/nonexistent/project"})
        assert r["isError"] and "No open project contains" in text(r), r

    def cancel_while_building():
        # The copy has never been built: the test run's build takes a while.
        rid = c.start("tools/call", {"name": "RunAllTests", "arguments": {}}, progress="cancel-me")
        deadline = time.time() + 120
        while not c.progress("cancel-me") and time.time() < deadline:
            time.sleep(0.1)
        assert c.progress("cancel-me"), "progress before the cancel"
        c.cancel(rid)
        z.wait_for(lambda s: s["phase"] == "Stopped", timeout=15, interval=0.3)
        assert c.response(rid, timeout=3) is None, "a cancelled request gets no response"
        assert c.call("GetBuildLog")["structuredContent"]["phase"] == "stopped"

    def build_with_an_error():
        broken = os.path.join(project, "Sources", "ZKit", "Broken.swift")
        with open(broken, "w") as f:
            f.write('let broken: Int = "text"\n')
        try:
            s = broken_build(broken)
        finally:
            if os.path.exists(broken):
                os.remove(broken)
        s2 = c.call("BuildProject")["structuredContent"]
        assert s2["success"] and s2["status"] == "succeeded" and s2["errors"] == [] and s2["log_path"] != s["log_path"], s2

    def broken_build(broken):
        r = c.call("BuildProject", progress="b")
        s = r["structuredContent"]
        assert not r["isError"] and not s["success"] and s["status"] == "failed" and s["error_count"] >= 1, s
        error = next(e for e in s["errors"] if (e["path"] or "").endswith("Broken.swift"))
        assert error["line"] == 1 and error["severity"] == "error" and error["column"], error
        steps = [p["progress"] for p in c.progress("b")]
        assert steps and steps == sorted(set(steps)), steps
        assert s["log_path"] and os.path.exists(s["log_path"]), s
        issues = c.call("XcodeListNavigatorIssues", {"severity": "error"})["structuredContent"]
        assert any((i["path"] or "").endswith("Broken.swift") for i in issues["issues"]), issues
        only = c.call("XcodeListNavigatorIssues", {"path": "Sources/ZKit/Broken.swift"})["structuredContent"]["issues"]
        assert only and all(i["path"].endswith("Broken.swift") for i in only), only
        log = c.call("GetBuildLog", {"raw": True, "filter": "error:"})["structuredContent"]
        assert log["lines"] and all("error:" in l for l in log["lines"]) and log["reports"][0]["success"] is False, log
        return s

    def tests_with_results():
        r = c.call("RunAllTests", progress="t")["structuredContent"]
        names = {t["name"] for t in r["tests"]}
        assert r["success"] and r["failed"] == 0 and r["total"] >= 4, r
        assert {"plainGreeting()", "GreeterXCTests/testPlainGreeting"} <= names, names
        assert all(t["status"] == "passed" for t in r["tests"]), r["tests"]
        one = c.call("RunSomeTests", {"tests": ["ZKitTests/excitedGreeting()"]})["structuredContent"]
        assert [t["name"] for t in one["tests"]] == ["excitedGreeting()"] and one["passed"] == 1, one
        failing = os.path.join(project, "Tests", "ZKitTests", "FailingTests.swift")
        with open(failing, "w") as f:
            f.write("import Testing\n\n@Test func failsOnPurpose() {\n    #expect(1 == 2)\n}\n")
        try:
            r = c.call("RunAllTests")["structuredContent"]
            assert not r["success"] and r["failed"] == 1 and r["status"] == "failed", r
            assert any(t["name"] == "failsOnPurpose()" and t["status"] == "failed" for t in r["tests"]), r["tests"]
            assert any((f["path"] or "").endswith("FailingTests.swift") and f["line"] == 4 for f in r["failures"]), r["failures"]
        finally:
            os.remove(failing)
        r = c.call("RunSomeTests", {"tests": ["NoSuchTest"]})
        assert r["isError"] and "no test named NoSuchTest" in text(r), r
        listed = c.call("GetTestList")["structuredContent"]
        found = {t["name"]: t for t in listed["tests"]}
        assert listed["loaded"] and listed["last_run"] == "All tests", listed
        plain = found["ZKitTests/plainGreeting()"]
        assert plain["status"] == "passed" and plain["path"].endswith("GreeterTests.swift") and plain["line"] == 5, plain
        assert found["ZKitTests/GreeterXCTests/testPlainGreeting"]["kind"] == "test", found.keys()
        assert found["ZKitTests/GreeterXCTests"]["kind"] == "suite", found.keys()

    def run_and_console():
        r = c.call("RunProject", {"wait_seconds": 2})["structuredContent"]
        assert r["success"] and r["status"] in ("finished", "running") and "tick 3" in r["console"], r
        lines = c.call("GetConsoleOutput", {"max_lines": 50})["structuredContent"]["lines"]
        assert "tick 1" in lines and any(l.startswith("Program ended") for l in lines), lines
        assert "stopped" in c.call("StopProject")["structuredContent"]

    def profile_project():
        r = c.call("ProfileProject", {"time_limit_seconds": 2})["structuredContent"]
        assert r["success"] and r["template"] == "Time Profiler" and r["opened_in_instruments"], r
        assert r["trace"].startswith(os.path.join(config, "traces")) and xctrace.opened() == [r["trace"]], r
        summary = r["summary"]
        assert summary["process"] == "zhello" and summary["functions"][0]["function"] == "Greeter.greeting.getter", summary
        assert summary["functions"][0]["self_percent"] == 66.67, summary
        record = xctrace.records()[-1]
        assert record[record.index("--time-limit") + 1] == "2s" and record[record.index("--") + 1].lower().endswith("/release/zhello"), record
        try:
            c.call("ProfileProject", {"time_limit_seconds": 0})
            raise AssertionError("a time limit of 0 should be refused")
        except McpError as e:
            assert e.code == -32602, e.code

    def editor_tools():
        r = c.call("OpenFile", {"path": "Sources/ZKit/Greeter.swift", "line": 9, "column": 5})["structuredContent"]
        assert r["path"] == greeter, r
        z.wait_for(lambda s: (s["active"] or {}).get("path") == greeter and s["active"]["line"] == 9, timeout=10, interval=0.3)
        z.run("keys shift-down", "wait 200")
        e = c.call("XcodeGetCurrentFile", {"include_text": True})["structuredContent"]
        lines = open(greeter).read().splitlines(keepends=True)
        assert e["path"] == greeter and e["language"] == "swift" and not e["unsaved_changes"], e
        assert e["selection"] == {"start": {"line": 9, "column": 5}, "end": {"line": 10, "column": 5}}, e["selection"]
        assert e["selected_text"] == lines[8][4:] + lines[9][:4] and e["text"] == "".join(lines), e["selected_text"]
        # What the editor shows, unsaved edits included.
        z.run("type // mcp", "wait 200")
        e = c.call("XcodeGetCurrentFile", {"include_text": True})["structuredContent"]
        assert e["unsaved_changes"] and "// mcp" in e["text"] and e["selected_text"] == "", e
        for _ in range(10):
            z.run("keys cmd-z", "wait 150")
            if not c.call("XcodeGetCurrentFile")["structuredContent"]["unsaved_changes"]:
                break
        assert not c.call("XcodeGetCurrentFile")["structuredContent"]["unsaved_changes"], "undone"
        r = c.call("OpenFile", {"path": "Sources/Nope.swift"})
        assert r["isError"] and "No file at" in text(r), r

    def debugger_paused_at_a_breakpoint():
        r = c.call("EvaluateInDebugger", {"command": "p 1"})
        assert r["isError"] and "Nothing is being debugged" in text(r), r
        line = line_of(main_swift, 'print("tick')
        z.run(f"open {main_swift}:{line}", "wait 300", "keys cmd-\\")
        try:
            r = c.call("RunProject", {"wait_seconds": 0})["structuredContent"]
            assert r["status"] == "running" and r["debugging"], r
            stopped(z, variables=False)
            e = c.call("EvaluateInDebugger", {"command": "p i"})["structuredContent"]
            assert e["state"] == "Stopped" and e["frame"]["line"] == line and e["frame"]["path"].endswith("main.swift"), e
            assert e["output"].strip().endswith(" 1"), e
            # Shown in the console as if typed at its prompt.
            assert "(lldb) p i" in z.dump()["console_tail"]
            assert c.call("StopProject")["structuredContent"]["stopped"]
            z.wait_for(lambda s: not s["debugger"]["state"].startswith(("Running", "Stopped", "Starting")), timeout=30)
        finally:
            z.run(f"open {main_swift}:{line}", "wait 300", "keys cmd-\\")

    def window_screenshot():
        r = c.call("TakeScreenshot")
        s = r["structuredContent"]
        image = next(b for b in r["content"] if b["type"] == "image")
        png = base64.b64decode(image["data"])
        assert image["mimeType"] == "image/png" and png[:8] == b"\x89PNG\r\n\x1a\n", image["mimeType"]
        # A pixel per point: the window's size, not a Retina capture's.
        assert s["target"] == "window" and 700 <= s["width"] <= 2000 and 400 <= s["height"] <= 1400, s
        r = c.call("TakeScreenshot", {"target": "simulator"})
        assert r["isError"] and "No simulator is booted" in text(r), r
        r = c.call("TakeScreenshot", {"target": "simulator", "udid": "mac"})
        assert r["isError"] and "isn’t a simulator" in text(r), r

    def agent(s=None):
        return (s or z.dump())["agent"]

    def entries(s=None):
        return agent(s)["entries"]

    def pending(s=None):
        return [e for e in entries(s) if e["kind"] == "tool" and (e["permission"] or {}).get("pending")]

    def send(message):
        before = len(entries())
        z.run(f"agent send {message}")
        return before

    def answered(before, timeout=300):
        s = z.wait_for(lambda s: agent(s)["status"] == "idle" and len(entries(s)) > before
                       and entries(s)[-1]["kind"] == "assistant", timeout=timeout, interval=0.3)
        return entries(s)[-1]["text"]

    def asked(title):
        s = z.wait_for(lambda s: pending(s), timeout=30, interval=0.2)
        p = pending(s)[0]
        assert p["title"] == title and p["permission"]["options"] == ["Allow", "Always Allow", "Reject"], p
        return p

    def agent_panel_hands_over_the_tools():
        z.run("agent show", "wait 300")
        before = send("mcp XcodeListSchemes")
        reply = answered(before, timeout=60)
        assert reply.startswith("mcp XcodeListSchemes ok") and '"scheme":"zhello"' in reply, reply
        # Read-only tools aren't asked about.
        assert not any(e["kind"] == "tool" and e["title"].startswith("zcode:") for e in entries()), entries()
        with open(acp_log) as f:
            sent = [json.loads(l)["msg"] for l in f if json.loads(l)["dir"] == "in"]
        new = next(m for m in sent if m.get("method") == "session/new")["params"]
        server = new["mcpServers"][0]
        env = {e["name"]: e["value"] for e in server["env"]}
        assert server["name"] == "zcode" and env["ZCODE_MCP_SOCKET"] == sock, server

    def agent_tools_that_change_things_ask_first():
        before = send("mcp BuildProject")
        call = asked("zcode: Build “zhello”")
        z.run("agent allow")
        reply = answered(before)
        assert reply.startswith("mcp BuildProject ok") and '"success":true' in reply, reply
        status = next(e for e in entries() if e.get("id") == call["id"])["status"]
        assert status == "Completed", status
        before = send("mcp StopProject")
        asked("zcode: Stop")
        z.run("agent reject")
        reply = answered(before, timeout=30)
        assert reply == "mcp StopProject error: The user didn’t allow StopProject in zcode.", reply
        # Always Allow holds for the thread, not the next one.
        before = send("mcp StopProject")
        asked("zcode: Stop")
        z.run("agent allow always")
        assert answered(before, timeout=30).startswith("mcp StopProject ok")
        before = send("mcp StopProject")
        assert answered(before, timeout=30).startswith("mcp StopProject ok") and not pending()
        z.run("agent new", "wait 300")
        send("mcp StopProject")
        asked("zcode: Stop")
        z.run("agent allow")
        z.wait_for(lambda s: agent(s)["status"] == "idle" and not pending(s), timeout=30, interval=0.3)

    try:
        check("MCP: the socket is the user's alone (0600, peer checked); a silent client doesn't block it", private_socket)
        check("MCP: initialize in 2025-06-18 and 2025-11-25, tools/list, errors", handshake_revisions)
        check("MCP: 2026-07-28 (server/discover, the revision per request, an unknown one refused)", modern_revision)
        check("MCP: schemes and run destinations listed and switched; a scheme, destination or project that isn't there", schemes_and_destinations)
        check("MCP: a test run cancelled while it builds stops, with no response", cancel_while_building)
        check("MCP: a build with an error (progress, structured errors, the Issue navigator, the log), then fixed", build_with_an_error)
        check("MCP: RunAllTests and RunSomeTests with per-test results, a failure with its line; GetTestList", tests_with_results)
        check("MCP: run and the console, stop", run_and_console)
        check("MCP: ProfileProject records a Release build with a time limit and returns the trace and heaviest functions", profile_project)
        check("MCP: OpenFile and XcodeGetCurrentFile (selection, unsaved edits)", editor_tools)
        check("MCP: EvaluateInDebugger at a breakpoint, shown in the console", debugger_paused_at_a_breakpoint)
        check("MCP: a window screenshot (PNG, a pixel per point); no simulator, not a device", window_screenshot)
        check("MCP: the agent panel hands its agent zcode's server; read-only tools aren't asked about", agent_panel_hands_over_the_tools)
        check("MCP: the agent's BuildProject and StopProject ask in the panel: Allow, Reject, Always Allow for the thread", agent_tools_that_change_things_ask_first)
    finally:
        for each in clients:
            each.close()
        z.quit()
        shutil.rmtree(config, ignore_errors=True)


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


def packaging_checks(samples):
    """zcode as a Mac app (scripts/package.py): the bundle and its Info.plist
    (the document types Finder's Open With lists, by inspection: no
    LaunchServices registration), an ad hoc signature and the dry runs of
    Developer ID signing and notarization; then, in a copy of the bundle
    launched from a folder of its own, what Finder hands over, the CLI link
    (ZCODE_CLI_DIR) and a launch through it, Open at Login (a file stands in
    for launchd: ZCODE_LOGIN_ITEM_FILE) and an update from a local feed
    (scripts/fake_update_feed.py) installed in the bundle's place."""
    out = tempfile.mkdtemp(prefix="zcode-e2e-package-")
    package_py = os.path.join(ROOT, "scripts", "package.py")

    def package(*args, out=out):
        return subprocess.run([sys.executable, package_py, *args, "--out", out], check=True, capture_output=True, text=True)

    app = os.path.join(out, "zcode.app")
    version = subprocess.run([BINARY, "--version"], capture_output=True, text=True, check=True).stdout.split()[1]

    def bundle_is_built():
        package("bundle", "--quiet")
        package("sign", "--adhoc", "--quiet")
        with open(os.path.join(app, "Contents", "Info.plist"), "rb") as f:
            info = plistlib.load(f)
        assert info["CFBundleShortVersionString"] == version and info["CFBundleVersion"] == version, info
        assert info["CFBundleIdentifier"] == "dev.zcode.zcode" and info["CFBundleExecutable"] == "zcode", info
        assert info["LSApplicationCategoryType"] == "public.app-category.developer-tools", info
        types = {t["CFBundleTypeName"]: t for t in info["CFBundleDocumentTypes"]}
        assert types["Xcode Project"]["LSItemContentTypes"] == ["com.apple.xcode.project"], types
        assert types["Xcode Workspace"]["LSItemContentTypes"] == ["com.apple.dt.document.workspace"], types
        assert types["Swift Source"]["LSItemContentTypes"] == ["public.swift-source"], types
        assert types["Folder"]["LSItemContentTypes"] == ["public.folder"], types
        # Xcode stays the default opener; zcode is in Open With.
        assert all(t["LSHandlerRank"] == "Alternate" for t in types.values()), types
        assert {u["UTTypeIdentifier"] for u in info["UTImportedTypeDeclarations"]} == {"com.apple.xcode.project", "com.apple.dt.document.workspace"}
        assert os.path.getsize(os.path.join(app, "Contents", "Resources", "zcode.icns")) > 100_000
        with open(os.path.join(app, "Contents", "Library", "LaunchAgents", "dev.zcode.zcode.background.plist"), "rb") as f:
            agent = plistlib.load(f)
        assert agent["BundleProgram"] == "Contents/MacOS/zcode" and agent["ProgramArguments"][-1] == "--background", agent
        assert agent["Label"] == "dev.zcode.zcode.background" and agent["RunAtLoad"] and agent["AssociatedBundleIdentifiers"] == ["dev.zcode.zcode"], agent
        subprocess.run(["codesign", "--verify", "--deep", "--strict", app], check=True, capture_output=True)
        signed = subprocess.run(["codesign", "-dv", "--verbose=2", app], capture_output=True, text=True).stderr
        assert "Identifier=dev.zcode.zcode" in signed and "flags=0x10002(adhoc,runtime)" in signed, signed
        # The bundle's binary is zcode.
        out_version = subprocess.run([os.path.join(app, "Contents", "MacOS", "zcode"), "--version"], capture_output=True, text=True).stdout
        assert out_version.strip() == f"zcode {version}", out_version
        # Developer ID signing and notarization only print their commands
        # here (no Developer ID certificate on this Mac); nothing changes.
        before = tree_snapshot(app)
        dry = package("sign", "--dry-run").stdout + package("notarize", "--dry-run").stdout
        assert "(dry run) codesign --force --options runtime --timestamp --sign Developer ID Application" in dry, dry
        assert "(dry run) xcrun notarytool submit" in dry and "--keychain-profile" in dry and "(dry run) xcrun stapler staple" in dry, dry
        assert tree_snapshot(app) == before
        assert not os.path.exists(os.path.join(out, "zcode-notarize.zip"))
        # The archives and the feed's entry.
        package("zip", "--quiet")
        package("appcast", "--url", "https://example.invalid/releases/", "--quiet")
        with open(os.path.join(out, "appcast.json")) as f:
            feed = json.load(f)
        archive = os.path.join(out, f"zcode-{version}.zip")
        digest = subprocess.run(["shasum", "-a", "256", archive], capture_output=True, text=True).stdout.split()[0]
        assert feed == {"version": version, "url": f"https://example.invalid/releases/zcode-{version}.zip", "sha256": digest,
                        "length": os.path.getsize(archive), "minimum_system": "14.0", "notes": ""}, feed

    check("Packaging: zcode.app from scripts/package.py (Info.plist: version, document types, the agent; icon; ad hoc signature; dry runs; zip and appcast)", bundle_is_built)

    def cold_launch_of_the_bundle():
        # Alternating rounds, so a loaded machine weighs on both.
        bare, bundled = [], []
        for _ in range(3):
            bare.append(median_launch([BINARY, "--startup-bench"], runs=3)[0])
            bundled.append(median_launch([os.path.join(app, "Contents", "MacOS", "zcode"), "--startup-bench"], runs=3)[0])
        bare_ms, bundle_ms = sorted(bare)[1], sorted(bundled)[1]
        native = f"; the minimal native AppKit app took {COLD_LAUNCH['native_ms']:.0f} ms" if "native_ms" in COLD_LAUNCH else ""
        print(f"    content on screen, medians of 3 rounds: bare binary {bare_ms:.0f} ms, zcode.app {bundle_ms:.0f} ms{native}")
        # AppKit's launch work for a bundled app (the Writing Tools library
        # it loads for the Edit menu) costs the bundle ~15–25 ms the bare
        # binary doesn't pay; nothing of zcode's may add to that.
        assert bundle_ms < bare_ms + 40, f"the bundle ({bundle_ms:.0f} ms) should launch within 40 ms of the bare binary ({bare_ms:.0f} ms)"

    check("Packaging: cold launch of zcode.app's binary is within 40 ms of the bare binary's", cold_launch_of_the_bundle)

    if not os.path.exists(app):
        return
    # A copy of the bundle in a folder of its own stands in for /Applications.
    apps = os.path.realpath(tempfile.mkdtemp(prefix="zcode-e2e-apps-"))
    installed = os.path.join(apps, "zcode.app")
    subprocess.run(["ditto", app, installed], check=True)
    installed_binary = os.path.join(installed, "Contents", "MacOS", "zcode")
    bin_dir = os.path.join(apps, "bin")
    login_file = os.path.join(apps, "login-item")
    feed_dir = tempfile.mkdtemp(prefix="zcode-e2e-feed-")
    port_file, requests_log = os.path.join(feed_dir, "port"), os.path.join(feed_dir, "requests.jsonl")
    server = subprocess.Popen([sys.executable, os.path.join(ROOT, "scripts", "fake_update_feed.py"), "--dir", feed_dir,
                               "--port-file", port_file, "--log", requests_log], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.time() + 15
    while not os.path.exists(port_file) and time.time() < deadline:
        time.sleep(0.1)
    with open(port_file) as f:
        port = int(f.read())
    feed_url = f"http://127.0.0.1:{port}/appcast.json"
    config = tempfile.mkdtemp(prefix="zcode-e2e-package-config-")
    env = {"ZCODE_CONFIG_DIR": config, "ZCODE_UPDATE_FEED": feed_url, "ZCODE_CLI_DIR": bin_dir, "ZCODE_LOGIN_ITEM_FILE": login_file}
    recorded = os.path.join(apps, "commands.jsonl")
    env["ZCODE_XCODE_COMMANDS"] = recorded

    def commands_recorded():
        try:
            with open(recorded) as f:
                return [json.loads(l) for l in f]
        except FileNotFoundError:
            return []

    def requests():
        try:
            with open(requests_log) as f:
                return [json.loads(l)["path"] for l in f]
        except FileNotFoundError:
            return []

    # The next version, as a release: the bundle again with its version
    # bumped, signed ad hoc as the running one is, zipped, in the feed.
    next_version = version.rsplit(".", 1)[0] + "." + str(int(version.rsplit(".", 1)[1]) + 1)
    stage = os.path.join(feed_dir, "stage")
    os.makedirs(stage)
    subprocess.run(["ditto", app, os.path.join(stage, "zcode.app")], check=True)
    info_path = os.path.join(stage, "zcode.app", "Contents", "Info.plist")
    with open(info_path, "rb") as f:
        info = plistlib.load(f)
    info["CFBundleShortVersionString"] = info["CFBundleVersion"] = next_version
    with open(info_path, "wb") as f:
        plistlib.dump(info, f)
    package("sign", "--adhoc", "--quiet", out=stage)
    archive = os.path.join(feed_dir, f"zcode-{next_version}.zip")
    subprocess.run(["ditto", "-c", "-k", "--keepParent", os.path.join(stage, "zcode.app"), archive], check=True)
    digest = subprocess.run(["shasum", "-a", "256", archive], capture_output=True, text=True).stdout.split()[0]

    def set_feed(**entry):
        release = {"version": next_version, "url": f"http://127.0.0.1:{port}/zcode-{next_version}.zip", "sha256": digest, "notes": "e2e"}
        release.update(entry)
        with open(os.path.join(feed_dir, "appcast.json"), "w") as f:
            json.dump(release, f)

    project = os.path.join(samples, "..", "Packaged", "ZKit")
    project = os.path.realpath(project)
    shutil.copytree(os.path.join(samples, "ZKit"), project, ignore=shutil.ignore_patterns(".build"))
    spaced = os.path.join(os.path.dirname(project), "Hello Classic")
    shutil.copytree(os.path.join(samples, "HelloClassic"), spaced)
    z = Zcode(project, env, binary=installed_binary)
    try:
        def bundle_known():
            info = json.loads(z.run("app bundle")[-1])
            assert info["bundle"] == installed and info["version"] == version and not info["cli"], info
            assert info["executable"] == installed_binary and info["identifier"] == "dev.zcode.zcode", info
            # Menu items.
            keymap = json.loads(z.run("keymap")[-1])
            assert {"zcode::CheckForUpdates", "zcode::InstallCli", "zcode::ToggleOpenAtLogin"} <= set(keymap), sorted(keymap)

        check("Packaging: the bundle knows itself (path, version, identifier); the app menu's three items", bundle_known)

        def finder_opens():
            # What LaunchServices sends for a double-clicked project (a package: a folder URL).
            url = "file://" + spaced.replace(" ", "%20") + "/HelloClassic.xcodeproj/"
            opened = json.loads(z.run(f"app open-url {url}")[-1])["open"]
            assert opened == [project, spaced], opened

        check("Packaging: Finder's file URL (percent-encoded, a package's trailing slash) opens the project in a second window", finder_opens)

        def cli_installed():
            status = json.loads(z.run("app cli status")[-1])
            assert status["status"] == "missing" and status["link"] == os.path.join(bin_dir, "zcode"), status
            z.run("app cli install", "wait 500")
            status = json.loads(z.run("app cli status")[-1])
            assert status["status"] == "installed" and status["target"] == installed_binary, status
            assert os.readlink(os.path.join(bin_dir, "zcode")) == installed_binary
            # Through the link, a path goes to the running zcode (its socket is
            # in the run's configuration folder) and the shell gets its prompt
            # back at once.
            shell_env = {k: v for k, v in dict(os.environ, **env).items() if k != "ZCODE_DEBUG_DIR"}
            started = time.time()
            subprocess.run([os.path.join(bin_dir, "zcode"), project], env=shell_env, check=True, timeout=10)
            assert time.time() - started < 2, "the link should hand the path over and return"
            assert commands_recorded() == [], commands_recorded()
            # With no zcode running, the link starts the app through
            # LaunchServices (recorded here, so nothing opens on the screen).
            alone = dict(shell_env, ZCODE_CONFIG_DIR=tempfile.mkdtemp(prefix="zcode-e2e-cli-config-"))
            subprocess.run([os.path.join(bin_dir, "zcode"), project], env=alone, check=True, timeout=10)
            subprocess.run([os.path.join(bin_dir, "zcode"), "--background"], env=alone, check=True, timeout=10)
            assert commands_recorded() == [["/usr/bin/open", "-a", installed, "--args", project],
                                           ["/usr/bin/open", "-a", installed, "--args", "--background"]], commands_recorded()
            # Installing again replaces the link.
            z.run("app cli install", "wait 500")
            assert json.loads(z.run("app cli status")[-1])["status"] == "installed"

        check("Packaging: Install CLI links zcode into the folder (ZCODE_CLI_DIR); the link hands a path to the running zcode or starts the app", cli_installed)

        def login_item_toggles():
            status = json.loads(z.run("app login-item status")[-1])
            assert status["status"] == "not registered", status
            assert not json.loads(z.run("app login-item menu")[-1])["checked"]
            z.run("app login-item toggle", "wait 500")
            assert json.loads(z.run("app login-item menu")[-1])["checked"]
            assert json.loads(z.run("app login-item status")[-1])["status"] == "enabled"
            assert open(login_file).read() == "on"
            assert "zcode::ToggleOpenAtLogin" in z.dump()["settings"]["menu_checked"]
            z.run("action zcode::ToggleOpenAtLogin", "wait 500")
            assert not json.loads(z.run("app login-item menu")[-1])["checked"]
            assert open(login_file).read() == "off"
            assert "zcode::ToggleOpenAtLogin" not in z.dump()["settings"]["menu_checked"]

        check("Packaging: Open at Login toggles the login item (a file stands in for launchd) and the menu's checkmark follows", login_item_toggles)

        def settled():
            deadline = time.time() + 60
            while time.time() < deadline:
                status = json.loads(z.run("app updates status")[-1])
                if status["state"] in ("ready", "failed", "up to date", "available"):
                    return status
                time.sleep(0.3)
            raise TimeoutError(status)

        def update_installs():
            status = json.loads(z.run("app updates status")[-1])
            assert status["state"] == "idle" and status["current"] == version and status["feed"] == feed_url, status
            # A wrong checksum: nothing is installed.
            set_feed(sha256="0" * 64)
            z.run("app updates check")
            status = settled()
            assert status["state"] == "failed" and "SHA-256" in status["detail"], status
            assert requests()[-2:] == ["/appcast.json", f"/zcode-{next_version}.zip"], requests()
            with open(os.path.join(installed, "Contents", "Info.plist"), "rb") as f:
                assert plistlib.load(f)["CFBundleShortVersionString"] == version
            # The release: downloaded, checked, unpacked and swapped in.
            set_feed()
            z.run("app updates check")
            status = settled()
            assert status["state"] == "ready" and status["version"] == next_version, status
            with open(os.path.join(installed, "Contents", "Info.plist"), "rb") as f:
                assert plistlib.load(f)["CFBundleShortVersionString"] == next_version
            subprocess.run(["codesign", "--verify", "--deep", "--strict", installed], check=True, capture_output=True)
            previous = os.path.join(config, "updates", f"zcode-{version}-previous.app")
            with open(os.path.join(previous, "Contents", "Info.plist"), "rb") as f:
                assert plistlib.load(f)["CFBundleShortVersionString"] == version
            assert not [n for n in os.listdir(apps) if n.startswith(".zcode-update")], os.listdir(apps)
            # Checking again offers the restart rather than installing twice.
            z.run("app updates check", "wait 300")
            assert json.loads(z.run("app updates status")[-1])["state"] == "ready"
            # Restart Now: the relaunch waits for this process, then opens the bundle.
            try:
                z.run("app updates restart")
            except RuntimeError:
                pass  # quit before the command's log line was counted
            z.proc.wait(timeout=15)
            relaunch = commands_recorded()[-1]
            assert relaunch[:2] == ["/bin/sh", "-c"] and f'/usr/bin/open -a "{installed}"' in relaunch[2] and str(z.proc.pid) in relaunch[2], relaunch

        check("Packaging: Check for Updates… fails a bad checksum, installs the release in the bundle's place, and Restart Now relaunches it", update_installs)
    finally:
        z.quit()

    def updated_app_runs():
        z2 = Zcode(project, env, binary=installed_binary)
        try:
            info = json.loads(z2.run("app bundle")[-1])
            assert info["version"] == next_version and info["bundle"] == installed, info
            z2.run("app updates check")
            deadline = time.time() + 60
            while time.time() < deadline:
                status = json.loads(z2.run("app updates status")[-1])
                if status["state"] != "checking":
                    break
                time.sleep(0.3)
            assert status["state"] == "up to date" and status["current"] == next_version, status
            assert subprocess.run([installed_binary, "--version"], capture_output=True, text=True).stdout.strip() == f"zcode {version}"
        finally:
            z2.quit()

    check("Packaging: the installed update runs and finds itself up to date", updated_app_runs)
    server.kill()
    server.wait()
    shutil.rmtree(feed_dir, ignore_errors=True)
    shutil.rmtree(apps, ignore_errors=True)
    shutil.rmtree(config, ignore_errors=True)
    shutil.rmtree(out, ignore_errors=True)


def warm_open(samples, background=False, limit_ms=150):
    """Starts a resident zcode (a regular window, or `--background`), then
    times `zcode <project>` through it."""
    # The run's configuration folder holds the socket (never the user's).
    sock = os.path.join(CONFIG_DIR, "zcode.sock")
    log = os.path.join(WORK, "instance.log")
    env = {k: v for k, v in os.environ.items() if k not in ("ZCODE_DEBUG_DIR",)}
    env["ZCODE_TRACE_STARTUP"] = "1"
    with open(log, "w") as out:
        server = subprocess.Popen([BINARY] + (["--background"] if background else []), env=env, stdout=out, stderr=out)
    try:
        deadline = time.time() + 15
        while not os.path.exists(sock) and time.time() < deadline:
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
        if os.path.exists(sock):
            os.remove(sock)


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


def settings_checks(samples):
    """File-based settings on a copy of ZKit, in the run's own configuration
    folder (ZCODE_CONFIG_DIR): ⌘, opening settings.json and edits applying as
    soon as it's saved; a file with a mistake keeping its last good settings
    and saying where; a project's .zcode/settings.json over the user's; the
    Editor menu's toggles writing the file and their checkmarks; format and
    whitespace on save; the terminal's shell; keymap.json with contexts,
    `null` and Unbind; themes as JSON and Select Theme…; Xcode's themes
    (a fixture's and Xcode's own, only read) and key bindings imported; the
    session's old toggles moved into settings.json once; the default files
    read-only."""
    project = os.path.realpath(os.path.join(WORK, "SettingsKit"))
    shutil.copytree(os.path.join(samples, "ZKit"), project, ignore=shutil.ignore_patterns(".build"))
    greeter = os.path.join(project, "Sources", "ZKit", "Greeter.swift")
    fixtures = os.path.join(ROOT, "tests", "fixtures", "xcode")
    user_config = os.path.expanduser("~/.config/zcode")
    before_user = sorted(os.listdir(user_config)) if os.path.isdir(user_config) else None
    before_user_stamps = {n: os.stat(os.path.join(user_config, n)).st_mtime for n in (before_user or [])}
    z = Zcode(project)
    paths = json.loads(z.run("settings paths")[-1])
    settings_file, keymap_file, themes_dir = paths["settings"], paths["keymap"], paths["themes"]

    def settings(s=None):
        return (s or z.dump())["settings"]

    def write(path, text):
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(text)

    def write_settings(text):
        write(settings_file, text)

    def wait_settings(predicate, timeout=10):
        """Live reload: zcode notices the saved file by itself."""
        return z.wait_for(lambda s: predicate(settings(s)), timeout=timeout, interval=0.3)

    def own_config_folder():
        assert paths["config_dir"] == CONFIG_DIR and paths["session"].startswith(CONFIG_DIR), paths
        assert not paths["settings"].startswith(os.path.expanduser("~/.config")), paths

    def cmd_comma_and_live_reload():
        z.run(f"open {greeter}", "wait 300", "keys cmd-,", "wait 400")
        s = z.dump()
        assert s["active"]["path"] == os.path.realpath(settings_file), s["active"]["path"]
        assert "// Your zcode settings" in s["active"]["text"], s["active"]["text"]
        z.run(f"open {greeter}", "wait 300")
        write_settings('// Mine.\n{\n  "tab_size": 2,\n  "hard_tabs": true,\n  "buffer_font_size": 15,\n  "soft_wrap": "editor_width",\n  "show_whitespaces": "all",\n  "gutter": { "line_numbers": false },\n}\n')
        s = wait_settings(lambda st: st["tab_size"] == 2)
        st = settings(s)
        assert st["active_editor"] == {"tab_size": 2, "hard_tabs": True, "soft_wrap": True, "line_numbers": False, "show_whitespaces": True}, st["active_editor"]
        assert s["font_size"] == 15 and st["problems"] == [], (s["font_size"], st["problems"])
        assert "zcode::ToggleSoftWrap" in st["menu_checked"] and "zcode::ToggleInvisibles" in st["menu_checked"], st["menu_checked"]
        # Tab types a tab now.
        original = z.dump()["active"]["text"]
        z.run("keys cmd-up end enter tab", "wait 200")
        assert z.dump()["active"]["line_text"] == "\t", repr(z.dump()["active"]["line_text"])
        z.run("keys cmd-z", "wait 200")
        if z.dump()["active"]["text"] != original:
            z.run("keys cmd-z", "wait 200")
        assert z.dump()["active"]["text"] == original
        write_settings("{}\n")
        s = wait_settings(lambda st: st["tab_size"] == 4)
        assert settings(s)["active_editor"]["line_numbers"] and s["font_size"] == 13, settings(s)

    def mistakes_keep_the_last_good_settings():
        write_settings('{ "tab_size": 3, "tab_szie": 5 }\n')
        st = settings(wait_settings(lambda st: st["tab_size"] == 3))
        assert [p["message"] for p in st["problems"]] == ["settings.json: unknown setting “tab_szie”"], st["problems"]
        write_settings('{\n  "tab_size": 6\n  "soft_wrap": "none"\n}\n')
        st = settings(wait_settings(lambda st: any(p["error"] for p in st["problems"])))
        assert st["tab_size"] == 3, "the last good settings stay"
        assert st["problems"][0]["message"].startswith("settings.json: line 3, column 3: expected `,`"), st["problems"]
        write_settings('{\n  "tab_size": 0\n}\n')
        st = settings(wait_settings(lambda st: any("1 to 16" in p["message"] for p in st["problems"])))
        assert st["problems"][0]["message"].startswith("settings.json: line 2, column 15: expected a number of columns"), st["problems"]
        write_settings("{}\n")
        wait_settings(lambda st: st["problems"] == [] and st["tab_size"] == 4)

    def project_settings_win():
        z.run("settings open project", "wait 300")
        project_file = os.path.join(project, ".zcode", "settings.json")
        assert z.dump()["active"]["path"] == project_file
        write(project_file, '{ "tab_size": 8, "theme": "Default Light", "vim_mode": true }\n')
        st = settings(wait_settings(lambda st: st["tab_size"] == 8))
        messages = [p["message"] for p in st["problems"]]
        assert any(m.endswith(".zcode/settings.json: “theme” is only read from your own settings file") for m in messages), messages
        assert not st["vim_mode"], "Vim mode is the user's"
        # A toggle the project sets is written there; others to the user's.
        z.run(f"open {greeter}", "wait 200", "settings set tab_size 7", "wait 300")
        assert '"tab_size": 7' in open(project_file).read()
        os.remove(project_file)
        st = settings(wait_settings(lambda st: st["tab_size"] == 4))
        assert st["problems"] == [], st["problems"]
        z.run("keys cmd-w", "wait 200")

    def toggles_write_the_file_with_checkmarks():
        z.run(f"open {greeter}", "wait 300")
        for action, key in [("ToggleInlayHints", "inlay_hints"), ("ShowAuthors", "show_authors"), ("ToggleInvisibles", "show_whitespaces"), ("ToggleSoftWrap", "soft_wrap")]:
            z.run(f"action zcode::{action}", "wait 200")
            st = settings()
            assert st[key] and f"zcode::{action}" in st["menu_checked"], (action, st)
            z.run(f"action zcode::{action}", "wait 200")
            st = settings()
            assert not st[key] and f"zcode::{action}" not in st["menu_checked"], (action, st)
        text = open(settings_file).read()
        assert '"inlay_hints": {\n    "enabled": false\n  }' in text and '"show_whitespaces": "none"' in text, text
        assert '"git": {\n    "show_authors": false\n  }' in text, text
        z.run("action zcode::InlineComparison", "wait 200")
        st = settings()
        assert "zcode::InlineComparison" in st["menu_checked"] and "zcode::SideBySideComparison" not in st["menu_checked"], st["menu_checked"]
        z.run("action zcode::SideBySideComparison", "keys cmd-+ cmd-+", "wait 300")
        assert settings()["buffer_font_size"] == 15, settings()["buffer_font_size"]
        assert '"buffer_font_size": 15\n' in open(settings_file).read(), open(settings_file).read()
        z.run("keys ctrl-cmd-0", "wait 300")
        assert z.dump()["font_size"] == 13

    def whitespace_and_format_on_save():
        write_settings('{ "remove_trailing_whitespace_on_save": true }\n')
        wait_settings(lambda st: st["remove_trailing_whitespace_on_save"])
        z.run(f"open {greeter}", "wait 300")
        z.run("keys cmd-up end space space space", "wait 100")
        assert z.dump()["active"]["line_text"] == "/// Builds greetings.   "
        z.run("keys cmd-s", "wait 500")
        assert open(greeter).read().startswith("/// Builds greetings.\n"), open(greeter).read()[:40]
        assert z.dump()["active"]["line_text"] == "/// Builds greetings.", "trimmed in the editor too"
        write_settings('{ "format_on_save": "on" }\n')
        wait_settings(lambda st: st["format_on_save"])
        z.wait_for(lambda s: s["lsp"] == "Ready", timeout=120)
        # Spacing sourcekit-lsp's formatter (swift-format) fixes.
        z.run("keys escape", f"open {greeter}:2", "wait 300", "keys end enter", "type let extra   =   1", "wait 200", "keys escape cmd-s")
        z.wait_for(lambda s: not s["active"]["dirty"], timeout=30)
        saved = open(greeter).read()
        assert "\n    let extra = 1\n" in saved, "formatted on save: " + saved
        assert z.dump()["active"]["line_text"].strip() == "let extra = 1", "and in the editor"
        # A file without a final newline keeps it that way.
        write_settings('{ "ensure_final_newline_on_save": false }\n')
        wait_settings(lambda st: not st["ensure_final_newline_on_save"] and not st["format_on_save"])
        z.run("keys cmd-down", "type // end", "keys cmd-s", "wait 500")
        assert open(greeter).read().endswith("// end"), open(greeter).read()[-40:]
        write_settings("{}\n")
        wait_settings(lambda st: not st["format_on_save"])
        with open(greeter, "w") as f:
            f.write(open(os.path.join(samples, "ZKit", "Sources", "ZKit", "Greeter.swift")).read())
        z.run("wait 3500")

    def terminal_shell_setting():
        write_settings('{ "terminal": { "shell": { "program": "/bin/bash", "args": ["--norc", "--noprofile"] } } }\n')
        wait_settings(lambda st: st["terminal_shell"] == ["/bin/bash", ["--norc", "--noprofile"]])
        z.run("terminal panel", "wait 1000", 'terminal type 0 echo "SHELL_IS=$0"\\n')
        z.wait_for(lambda s: any(l.strip() == "SHELL_IS=/bin/bash" for l in s["terminals"]["list"][0].get("lines") or []), timeout=20, interval=0.3)
        z.run("terminal close 0", "wait 300")
        write_settings("{}\n")
        wait_settings(lambda st: st["terminal_shell"] is None)

    def keymap_with_contexts():
        write(keymap_file, """[
  // Mine: a key for the Find navigator, one only in editors, one only in Vim's normal mode.
  { "bindings": { "cmd-shift-b": "zcode::ShowFindNavigator", "cmd-1": ["zed::Unbind", "zcode::ShowProjectNavigator"] } },
  { "context": "Editor", "bindings": { "ctrl-k": "DuplicateLine" } },
  { "context": "Editor && vim_mode == normal", "bindings": { "space f": "zcode::OpenQuickly" } },
  { "context": "Navigator && navigator == find", "bindings": { "ctrl-alt-n": "zcode::ShowProjectNavigator", "cmd-shift-k": null } },
  { "context": "Terminal", "bindings": { "cmd-j": "zcode::Nope" } }
]
""")
        st = settings(wait_settings(lambda st: st["user_bindings"] == 7))
        assert [p["message"] for p in st["problems"]] == ["keymap.json: “cmd-j”: no action named “zcode::Nope”"], st["problems"]
        keymap = json.loads(z.run("keymap")[-1])
        assert "cmd-shift-b" in keymap["zcode::ShowFindNavigator"], keymap["zcode::ShowFindNavigator"]
        z.run(f"open {greeter}", "wait 300", "keys cmd-shift-b", "wait 200")
        assert z.dump()["navigator"] == "Find", (z.dump()["navigator"], z.dump()["settings"]["key_context"])
        # ⌃⌥N, bound in the Find navigator only: nothing in the editor…
        z.run("pane focus 0", "wait 200")
        assert "Navigator navigator=find" not in z.dump()["settings"]["key_context"], z.dump()["settings"]["key_context"]
        z.run("keys ctrl-alt-n", "wait 200")
        assert z.dump()["navigator"] == "Find", "not in the editor"
        # …and back to the Project navigator from the Find navigator's field.
        z.run("keys cmd-shift-f", "wait 200")
        assert "Navigator navigator=find" in z.dump()["settings"]["key_context"], z.dump()["settings"]["key_context"]
        z.run("keys ctrl-alt-n", "wait 200")
        assert z.dump()["navigator"] == "Project", "the Navigator context's binding"
        z.run(f"open {greeter}", "wait 200", "keys cmd-shift-b", "wait 200")
        z.run("keys cmd-1", "wait 200")
        assert z.dump()["navigator"] == "Find", "Unbind took ⌘1 off Show Project Navigator"
        z.run(f"open {greeter}", "wait 200")
        text = z.dump()["active"]["text"]
        z.run("keys cmd-up ctrl-k", "wait 200")
        assert z.dump()["active"]["text"].count("/// Builds greetings.") == 2, "⌃K duplicated the line in the editor"
        z.run("keys cmd-z", "wait 200")
        assert z.dump()["active"]["text"] == text
        z.run("vim on", "wait 200", "keys escape space f", "wait 300")
        assert z.dump()["modal_open"], "space f opened Open Quickly in normal mode"
        z.run("keys escape", "vim off", "wait 200")
        write(keymap_file, "[]\n")
        wait_settings(lambda st: st["user_bindings"] == 0 and st["problems"] == [])
        z.run("keys cmd-1", "wait 200")
        assert z.dump()["navigator"] == "Project", "⌘1 is back"

    def json_themes_and_select_theme():
        write(os.path.join(themes_dir, "Harbor.json"), """{
  // A set of two, as gpui-component writes them.
  "name": "Harbor",
  "themes": [
    { "name": "Harbor Dark", "mode": "dark", "colors": { "selection.background": "#33507080" },
      "highlight": { "editor.background": "#102030ff", "syntax": { "keyword": { "color": "#ff6699ff", "font_weight": 700 } } } },
    { "name": "Harbor Light", "mode": "light", "highlight": { "editor.background": "#f0f4f8ff" } }
  ]
}
""")
        write(os.path.join(themes_dir, "zedish.json"), """{ "name": "Zedish", "themes": [{ "name": "Zedish Dark", "appearance": "dark",
  "style": { "background": "#202020", "text": "#e0e0e0", "editor.background": "#181818ff", "syntax": { "keyword": { "color": "#c678ddff" } } } }] }
""")
        write_settings('{ "theme": "Harbor Dark" }\n')
        st = settings(wait_settings(lambda st: st["theme"]["name"] == "Harbor Dark"))
        assert st["theme"]["editor_background"] == "#102030ff" and st["theme"]["keyword"] == "#ff6699ff", st["theme"]
        # (gpui-component keeps selections translucent: its own alpha.)
        assert st["theme"]["selection"].startswith("#335070") and st["theme"]["mode"] == "dark", st["theme"]
        # Editing the theme's file shows at once.
        path = os.path.join(themes_dir, "Harbor.json")
        write(path, open(path).read().replace("#102030ff", "#112233ff"))
        wait_settings(lambda st: st["theme"]["editor_background"] == "#112233ff")
        write_settings('{ "theme": "Zedish Dark" }\n')
        st = settings(wait_settings(lambda st: st["theme"]["name"] == "Zedish Dark"))
        assert st["theme"]["editor_background"] == "#181818ff" and st["theme"]["keyword"] == "#c678ddff", st["theme"]
        write_settings('{ "theme": "Nowhere" }\n')
        st = settings(wait_settings(lambda st: any("Nowhere" in p["message"] for p in st["problems"])))
        assert st["theme"]["name"] in ("Default Dark", "Default Light"), st["theme"]
        # Select Theme…: the light one, written to the settings.
        z.run("action zcode::SelectTheme", "wait 300", "type Harbor Light", "wait 200", "keys enter", "wait 500")
        st = settings()
        assert st["theme"]["name"] == "Harbor Light" and st["theme"]["mode"] == "light", st["theme"]
        assert '"theme": "Harbor Light"' in open(settings_file).read()
        write_settings("{}\n")
        wait_settings(lambda st: st["theme"]["name"] in ("Default Dark", "Default Light") and st["problems"] == [])

    def xcode_theme_import():
        z.run(f"settings import-theme {os.path.join(fixtures, 'Harbor Night.xccolortheme')}", "wait 500")
        written = os.path.join(themes_dir, "Harbor Night.json")
        assert os.path.exists(written), os.listdir(themes_dir)
        st = settings()
        assert st["theme"]["name"] == "Harbor Night" and st["theme"]["mode"] == "dark", st["theme"]
        assert st["theme"]["editor_background"] == "#101823ff" and st["theme"]["keyword"] == "#ff6699ff", st["theme"]
        assert st["theme"]["mono_font"] == "Menlo", st["theme"]
        # Xcode's own themes, read where Xcode keeps them.
        found = json.loads(z.run("settings xcode-themes")[-1])["themes"]
        built_in = [t["name"] for t in found if t["built_in"]]
        assert "Midnight" in built_in and "Default (Light)" in built_in, found
        xcode_dir = subprocess.run(["xcode-select", "-p"], capture_output=True, text=True).stdout.strip()
        midnight = os.path.join(os.path.dirname(xcode_dir), "SharedFrameworks/DVTUserInterfaceKit.framework/Versions/A/Resources/FontAndColorThemes/Midnight.xccolortheme")
        stamp = os.stat(midnight).st_mtime
        z.run(f"settings import-theme {midnight}", "wait 500")
        st = settings()
        assert st["theme"]["name"] == "Midnight" and st["theme"]["mode"] == "dark", st["theme"]
        assert os.stat(midnight).st_mtime == stamp, "Xcode's file was only read"
        write_settings("{}\n")
        wait_settings(lambda st: st["theme"]["name"] in ("Default Dark", "Default Light"))

    def xcode_key_bindings_import():
        write(keymap_file, '[\n  // Mine.\n  { "bindings": { "f5": "zcode::Run" } }\n]\n')
        z.run(f"settings import-keys {os.path.join(fixtures, 'Rebound.idekeybindings')}", "wait 500")
        blocks = json.loads(re_strip_comments(open(keymap_file).read()))
        assert blocks[0] == {"bindings": {"f5": "zcode::Run"}}, blocks
        assert blocks[1]["xcode_import"] == "Rebound.idekeybindings" and blocks[1]["bindings"]["alt-cmd-4"] == "zcode::ShowFindNavigator", blocks
        assert blocks[1]["bindings"]["cmd-4"] == ["zed::Unbind", "zcode::ShowFindNavigator"], blocks
        assert blocks[1]["bindings"]["cmd-5"] == ["zed::Unbind", "zcode::ShowIssueNavigator"], blocks
        assert blocks[2] == {"xcode_import": "Rebound.idekeybindings", "context": "Editor", "bindings": {"ctrl-shift-k": "input::DeleteToEndOfLine"}}, blocks
        settings(wait_settings(lambda st: st["user_bindings"] >= 5))
        z.run(f"open {greeter}", "wait 200", "keys cmd-1 cmd-4", "wait 200")
        assert z.dump()["navigator"] == "Project", "⌘4 was rebound in Xcode"
        z.run("keys alt-cmd-4", "wait 200")
        assert z.dump()["navigator"] == "Find"
        z.run("keys cmd-1 cmd-5", "wait 200")
        assert z.dump()["navigator"] == "Project", "Issues lost its shortcut in Xcode"
        # Again: the import's blocks are replaced, not added.
        z.run(f"settings import-keys {os.path.join(fixtures, 'Rebound.idekeybindings')}", "wait 500")
        assert len(json.loads(re_strip_comments(open(keymap_file).read()))) == 3
        # The user's own sets, read where Xcode keeps them (may be empty).
        for path in json.loads(z.run("settings xcode-key-sets")[-1])["sets"]:
            stamp = os.stat(path).st_mtime
            z.run(f"settings import-keys {path}", "wait 300")
            assert os.stat(path).st_mtime == stamp
        write(keymap_file, "[]\n")
        wait_settings(lambda st: st["user_bindings"] == 0)

    def default_files_read_only():
        z.run("settings open default-settings", "wait 300")
        a = z.dump()["active"]
        assert a["read_only"] and '"tab_size": 4' in a["text"], a
        z.run("keys cmd-w", "settings open default-keymap", "wait 300")
        a = z.dump()["active"]
        assert a["read_only"] and a["path"].endswith("default-keymap.json"), a
        z.run("keys cmd-w", "wait 200")

    def user_config_untouched():
        after = sorted(os.listdir(user_config)) if os.path.isdir(user_config) else None
        assert after == before_user, (before_user, after)
        for name, mtime in before_user_stamps.items():
            assert os.stat(os.path.join(user_config, name)).st_mtime == mtime, name

    try:
        check("Settings: e2e uses a configuration folder of its own (ZCODE_CONFIG_DIR)", own_config_folder)
        check("Settings: ⌘, opens settings.json; saved edits apply at once (tab size, hard tabs, font size, wrap, invisibles, line numbers)", cmd_comma_and_live_reload)
        check("Settings: a file with a mistake keeps its last good settings and says where", mistakes_keep_the_last_good_settings)
        check("Settings: a project's .zcode/settings.json wins over the user's, except the app's own settings", project_settings_win)
        check("Settings: the Editor menu's toggles and ⌘+ write settings.json, with checkmarks", toggles_write_the_file_with_checkmarks)
        check("Settings: trailing whitespace, final newline and format on save (sourcekit-lsp)", whitespace_and_format_on_save)
        check("Settings: terminal.shell starts the terminal's shell", terminal_shell_setting)
        check("Settings: keymap.json with contexts (Editor, vim_mode, Navigator), Unbind and null; mistakes reported", keymap_with_contexts)
        check("Settings: themes as JSON (gpui-component's and Zed's formats), live, and Select Theme…", json_themes_and_select_theme)
        check("Settings: Xcode themes import (a fixture, and Xcode's Midnight read in place)", xcode_theme_import)
        check("Settings: Xcode key bindings import into keymap.json (rebound, removed, text bindings; again replaces)", xcode_key_bindings_import)
        check("Settings: Open Default Settings and Default Key Bindings are read-only", default_files_read_only)
    finally:
        z.quit()
    check("Settings: the user's ~/.config/zcode is never touched", user_config_untouched)

    def session_toggles_migrate():
        config = tempfile.mkdtemp(prefix="zcode-e2e-migrate-")
        write(os.path.join(config, "support", "session.json"), json.dumps({"recent": [], "projects": {}, "font_size": 15.0, "inlay_hints": True, "vim_mode": True, "show_authors": True}))
        other = Zcode(project, {"ZCODE_CONFIG_DIR": config}, keep_vim=True)
        try:
            st = settings(other.dump())
            assert st["buffer_font_size"] == 15 and st["inlay_hints"] and st["vim_mode"] and st["show_authors"], st
            assert set(st["menu_checked"]) >= {"zcode::ToggleVimMode", "zcode::ShowAuthors", "zcode::ToggleInlayHints"}, st["menu_checked"]
            moved = json.loads(re_strip_comments(open(os.path.join(config, "settings.json")).read()))
            assert moved == {"buffer_font_size": 15.0, "inlay_hints": {"enabled": True}, "vim_mode": True, "git": {"show_authors": True}}, moved
            session = json.load(open(os.path.join(config, "support", "session.json")))
            assert not ({"font_size", "vim_mode", "inlay_hints", "show_authors"} & set(session)), session
        finally:
            other.quit()
            shutil.rmtree(config, ignore_errors=True)

    check("Settings: the session's old toggles move into settings.json once", session_toggles_migrate)


def re_strip_comments(text):
    """JSON with `//` comments (as zcode writes them) as plain JSON."""
    return "\n".join(l for l in text.splitlines() if not l.strip().startswith("//"))


# The cold launch's reference (the minimal AppKit app's median), once measured.
COLD_LAUNCH = {}


def median_launch(command, runs=5):
    """Launches `command` (a `--startup-bench` zcode, or the reference app)
    once to warm up and then `runs` times: the medians of content on screen
    and of the first frame, in ms, and the last run's JSON."""
    env = dict(os.environ, ZCODE_NEW_INSTANCE="1")
    subprocess.run(command, capture_output=True, env=env)
    samples = []
    for _ in range(runs):
        out = subprocess.run(command, capture_output=True, text=True, env=env).stdout
        samples.append(json.loads(out.strip().splitlines()[-1]))
    median = lambda key: sorted(r[key] for r in samples)[len(samples) // 2]
    return median("total_to_first_pixels_ms"), median("total_to_first_frame_ms"), samples[-1]


def main():
    if not os.path.exists(BINARY):
        sys.exit("build first: cargo build --release")
    sim = "--simulator" in sys.argv
    print("Cold launch (welcome window; the first launch after a rebuild includes macOS's binary scan):")

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
        COLD_LAUNCH["native_ms"] = native_ms
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

    print("Build server (a copy of samples/HelloZ, sourcekit-lsp through `zcode bsp`):")
    build_server_checks(samples)

    print("Physical devices (fake devicectl):")
    device_checks(samples)

    print("Group-based Xcode project (samples/HelloClassic):")
    z = Zcode(os.path.join(samples, "HelloClassic"))
    try:
        classic_project_checks(z, samples)
    finally:
        z.quit()

    print("Profile (copies of HelloClassic and, with --simulator, HelloZ; scripts/fake_xctrace.py):")
    profile_checks(samples, sim)

    print("Archive and Distribute App (copies of HelloClassic and HelloZ; uploads and notarization through scripts/fake_distribute.py):")
    archive_checks(samples)

    print("Asset catalogs (copies of HelloClassic and HelloZ with tests/fixtures/xcassets):")
    asset_catalog_checks(samples)

    print("SwiftUI previews (a copy of HelloZ; Xcode's commands recorded, never run):")
    preview_checks(samples)

    print("Property lists, Info and Signing & Capabilities (copies of HelloZ and HelloClassic):")
    plist_checks(samples)

    print("Packaging (scripts/package.py: the bundle, the CLI link, Open at Login, an update from scripts/fake_update_feed.py):")
    packaging_checks(samples)

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

    print("Web languages (a copy of samples/HelloWeb: a Swift package with a Vite app in web/):")
    web_language_checks(samples)

    print("Editor panes (a copy of samples/ZKit):")
    pane_checks(samples)

    print("Source control (a copy of samples/ZKit in a new repository, with a local origin):")
    git_checks(samples)

    print("Multibuffers (a copy of samples/ZKit in a new repository, with 400 notes):")
    multibuffer_checks(samples)

    print("Integrated terminal (a copy of samples/ZKit, `zsh -f`):")
    terminal_checks(samples)

    print("Vim mode (a copy of samples/ZKit):")
    vim_checks(samples)

    print("Long lines (a folder with a minified script):")
    long_line_checks()

    print("Settings, key bindings and themes (a copy of samples/ZKit):")
    settings_checks(samples)

    print("Agent panel (a copy of samples/ZKit, scripts/fake_acp_agent.py):")
    agent_checks(samples)

    print("zcode as an MCP server (a copy of samples/ZKit, scripts/mcp_client.py):")
    mcp_checks(samples)

    print("Predictive code completion (a copy of samples/ZKit, scripts/fake_anthropic.py):")
    prediction_checks(samples)

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
            pid = running_app_pid(z)
        finally:
            z.quit()
        # Quitting zcode ends the app it ran rather than leaving it to launchd.
        process_gone(pid)

    check("Schemes xcodebuild makes up keep their options in the session; ⌘R uses them; quitting ends the app", schemeless_project)

    shutil.rmtree(DEBUG_DIR, ignore_errors=True)
    shutil.rmtree(CONFIG_DIR, ignore_errors=True)
    shutil.rmtree(WORK, ignore_errors=True)
    print(f"\n{passed} passed, {failed} failed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

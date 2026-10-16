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
    ("cmd-.", "zcode::Stop", "Product ▸ Stop"),
    ("cmd-shift-k", "zcode::Clean", "Product ▸ Clean Build Folder"),
    ("ctrl-0", "zcode::ChooseScheme", "Product ▸ Scheme ▸ Choose Scheme"),
    ("ctrl-shift-0", "zcode::ChooseDestination", "Product ▸ Destination ▸ Choose Destination"),
    ("ctrl-cmd-y", "zcode::DebugContinue", "Debug ▸ Pause / Continue"),
    ("f6", "zcode::StepOver", "Debug ▸ Step Over"),
    ("f7", "zcode::StepInto", "Debug ▸ Step Into"),
    ("f8", "zcode::StepOut", "Debug ▸ Step Out"),
    ("cmd-\\", "zcode::ToggleBreakpoint", "Debug ▸ Breakpoints ▸ Create Breakpoint at Current Line"),
    ("cmd-y", "zcode::ToggleBreakpointsActive", "Debug ▸ Activate Breakpoints"),
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


def empty_trash_item(name):
    """Removes a test file Move to Trash put in ~/.Trash."""
    path = os.path.expanduser(os.path.join("~/.Trash", name))
    if os.path.exists(path):
        os.remove(path)


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
        check("Run on iOS Simulator (⌘R) with console", run_on_simulator)
        check("Debug on iOS Simulator (breakpoint stop)", debug_on_simulator)


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

    check("New File in a new folder → file reference, group and Sources build file", new_file)
    check("File inspector (⌥⌘1): Target Membership on and off", target_membership)
    check("Rename and move (Rename to Views/…) update references and groups", rename_and_move)
    check("Move to Trash removes the reference and build file", trash)
    check("Add Files (⌥⌘A): copy a folder in as a group, Sources + Resources", add_files)
    check("xcodebuild builds the edited project (⌘B)", builds)


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

    shutil.rmtree(DEBUG_DIR, ignore_errors=True)
    shutil.rmtree(WORK, ignore_errors=True)
    print(f"\n{passed} passed, {failed} failed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""A stand-in for `xcrun xctrace` in the e2e tests (zcode runs it when
ZCODE_XCTRACE names it): Instruments can't record without Developer Tools
access, and a real recording would open Instruments on the user's screen.
State lives in $FAKE_XCTRACE_DIR:

    calls.jsonl     every invocation's arguments and working directory
    app_pid         the pid of the Mac app `record` launched

`list templates` prints Xcode 27's standard templates. `record --launch`
launches a Mac app (the executable in a bundle) with the `--env`
variables, its output on xctrace's stdout with `--target-stdout -`, and
records until SIGINT (Stop), the `--time-limit` or the app's exit; then it
writes the `--output` trace (a folder, as a real one is) and exits. Like a
recording whose app outlives it, it leaves the app running: zcode has to
end it. Simulators and devices launch nothing. `export --toc` and `export
--xpath` answer with a Time Profiler table for Time Profiler traces, in
`xctrace export`'s format (ids and refs).
"""

import json
import os
import plistlib
import signal
import subprocess
import sys
import time

STATE = os.environ["FAKE_XCTRACE_DIR"]

TEMPLATES = """== Standard Templates ==
Activity Monitor
Allocations
Animation Hitches
App Launch
CPU Profiler
Leaks
Logging
Network
SwiftUI
System Trace
Time Profiler

== User Templates ==
Hitches and Leaks
"""


def option(args, name):
    return args[args.index(name) + 1] if name in args else None


def record(args):
    template = option(args, "--template")
    output = option(args, "--output")
    device = option(args, "--device")
    limit = option(args, "--time-limit")
    target = args[args.index("--") + 1]
    target_args = args[args.index("--") + 2:]
    env = dict(os.environ)
    for i, arg in enumerate(args[:args.index("--")]):
        if arg == "--env":
            key, _, value = args[i + 1].partition("=")
            env[key] = value
    print(f"Starting recording with the {template} template. Launching process: {os.path.basename(target)}.", flush=True)
    print("Ctrl-C to stop the recording", flush=True)
    stopped = []
    signal.signal(signal.SIGINT, lambda *_: stopped.append(True))
    app = None
    if device is None:
        executable = target
        if target.endswith(".app"):
            with open(os.path.join(target, "Contents", "Info.plist"), "rb") as f:
                name = plistlib.load(f)["CFBundleExecutable"]
            executable = os.path.join(target, "Contents", "MacOS", name)
        out = None if option(args, "--target-stdout") == "-" else subprocess.DEVNULL
        app = subprocess.Popen([executable] + target_args, env=env, stdout=out, stderr=out, start_new_session=True)
        with open(os.path.join(STATE, "app_pid"), "w") as f:
            f.write(str(app.pid))
    deadline = time.time() + float(limit.rstrip("s")) if limit else None
    while not stopped and (deadline is None or time.time() < deadline) and (app is None or app.poll() is None):
        time.sleep(0.05)
    print("Recording completed. Saving output file...", flush=True)
    os.makedirs(output, exist_ok=True)
    with open(os.path.join(output, "fake.json"), "w") as f:
        json.dump({"template": template, "target": target, "device": device}, f)
    print(f"Output file saved as: {output}", flush=True)


TOC = """<?xml version="1.0"?>
<trace-toc>
    <run number="1">
        <info>
            <target>
                <device platform="macOS" name="Mac" uuid="FAKE"/>
                <process type="launched" name="{process}" pid="4242"/>
            </target>
        </info>
        <data>
            <table schema="tick"/>
            {table}
        </data>
    </run>
</trace-toc>
"""

SAMPLES = """<?xml version="1.0"?>
<trace-query-result>
<node xpath='//trace-toc[1]/run[1]/data[1]/table[2]'><schema name="time-profile"><col><mnemonic>time</mnemonic><name>Sample Time</name><engineering-type>sample-time</engineering-type></col><col><mnemonic>thread</mnemonic><name>Thread</name><engineering-type>thread</engineering-type></col><col><mnemonic>weight</mnemonic><name>Weight</name><engineering-type>weight</engineering-type></col><col><mnemonic>stack</mnemonic><name>Backtrace</name><engineering-type>tagged-backtrace</engineering-type></col></schema>
<row><sample-time id="1" fmt="00:00.100.000">100000000</sample-time><thread id="2" fmt="Main Thread 0x1 ({process}, pid: 4242)"><tid id="3" fmt="0x1">1</tid><process id="4" fmt="{process} (4242)"><pid id="5" fmt="4242">4242</pid></process></thread><weight id="6" fmt="1.00 ms">1000000</weight><tagged-backtrace id="7"><backtrace id="8"><frame id="9" name="Greeter.greeting.getter" addr="0x100003f10"><binary id="10" name="{process}" path="/fake/{process}"/></frame><frame id="11" name="ContentView.body.getter" addr="0x100004a20"><binary ref="10"/></frame></backtrace></tagged-backtrace></row>
<row><sample-time id="12" fmt="00:00.101.000">101000000</sample-time><thread ref="2"/><weight ref="6"/><tagged-backtrace ref="7"/></row>
<row><sample-time id="13" fmt="00:00.102.000">102000000</sample-time><thread ref="2"/><weight ref="6"/><tagged-backtrace id="14"><backtrace id="15"><frame id="16" name="objc_msgSend" addr="0x18a000000"><binary id="17" name="libobjc.A.dylib" path="/usr/lib/libobjc.A.dylib"/></frame><frame ref="11"/></backtrace></tagged-backtrace></row>
</node></trace-query-result>
"""


def export(args):
    trace = option(args, "--input")
    try:
        with open(os.path.join(trace, "fake.json")) as f:
            recorded = json.load(f)
    except FileNotFoundError:
        print("Export failed: Document Missing Template Error", file=sys.stderr)
        sys.exit(1)
    # An app bundle or executable's name; a device app's bundle identifier's last part.
    target = recorded["target"]
    process = os.path.splitext(os.path.basename(target))[0] if "/" in target else target.rsplit(".", 1)[-1]
    sampled = recorded["template"] == "Time Profiler"
    if "--toc" in args:
        table = '<table schema="time-profile" target-pid="SINGLE"/>' if sampled else '<table schema="leaks"/>'
        print(TOC.format(process=process, table=table))
    elif sampled and 'schema="time-profile"' in (option(args, "--xpath") or ""):
        print(SAMPLES.format(process=process))
    else:
        print('<?xml version="1.0"?>\n<trace-query-result>\n</trace-query-result>')


def main():
    args = sys.argv[1:]
    with open(os.path.join(STATE, "calls.jsonl"), "a") as f:
        f.write(json.dumps({"args": args, "cwd": os.getcwd()}) + "\n")
    if args[:2] == ["list", "templates"]:
        print(TEMPLATES, end="")
    elif args[:1] == ["record"]:
        record(args)
    elif args[:1] == ["export"]:
        export(args)
    else:
        print(f"fake xctrace: unsupported: {args}", file=sys.stderr)
        sys.exit(64)


if __name__ == "__main__":
    main()

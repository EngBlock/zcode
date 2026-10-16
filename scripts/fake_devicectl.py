#!/usr/bin/env python3
"""A stand-in for `xcrun devicectl` in the e2e tests (zcode runs it when
ZCODE_DEVICECTL names it). State lives in $FAKE_DEVICECTL_DIR:

    devices.json    what `list devices` reports (a devicectl JSON document);
                    the tests rewrite it to plug and unplug devices
    locked          present while the device is locked
    processes.json  the app processes `launch` started
    calls.jsonl     every invocation's arguments and DEVICECTL_CHILD_ environment

`device process launch --console` prints what the sample app prints (its
launch line and its arguments and HELLOZ_MODE), then runs until SIGTERM, as
devicectl does while the app runs.
"""

import json
import os
import plistlib
import signal
import sys
import time

STATE = os.environ["FAKE_DEVICECTL_DIR"]
PID = 4242


def path(name):
    return os.path.join(STATE, name)


def option(args, name):
    return args[args.index(name) + 1] if name in args else None


def write_result(args, result=None, error=None):
    out = option(args, "--json-output")
    if not out:
        return
    document = {"info": {"arguments": ["devicectl"] + args, "outcome": "failed" if error else "success", "jsonVersion": 5}}
    if error:
        document["error"] = {"code": 1, "domain": "com.apple.dt.CoreDeviceError", "userInfo": {"NSLocalizedDescription": {"string": error}}}
    else:
        document["result"] = result or {}
    with open(out, "w") as f:
        json.dump(document, f)


def processes():
    try:
        with open(path("processes.json")) as f:
            return json.load(f)
    except FileNotFoundError:
        return []


def set_processes(running):
    with open(path("processes.json"), "w") as f:
        json.dump(running, f)


def installed_url(app_name):
    return f"file:///private/var/containers/Bundle/Application/FAKE-0000/{app_name}/"


def main():
    args = sys.argv[1:]
    with open(path("calls.jsonl"), "a") as f:
        env = {k: v for k, v in os.environ.items() if k.startswith("DEVICECTL_CHILD_")}
        f.write(json.dumps({"args": args, "env": env}) + "\n")
    command = [a for a in args if not a.startswith("-")][:3]

    if command[:2] == ["list", "devices"]:
        with open(path("devices.json")) as f:
            write_result(args, json.load(f)["result"])
    elif command[:3] == ["device", "install", "app"]:
        app = next(a for a in args if a.endswith(".app"))
        with open(os.path.join(app, "Info.plist"), "rb") as f:
            bundle_id = plistlib.load(f)["CFBundleIdentifier"]
        write_result(args, {"installedApplications": [
            {"bundleID": bundle_id, "installationURL": installed_url(os.path.basename(app))}]})
    elif command[:3] == ["device", "info", "lockState"]:
        write_result(args, {"passcodeRequired": os.path.exists(path("locked")), "unlockedSinceBoot": True})
    elif command[:3] == ["device", "info", "processes"]:
        write_result(args, {"runningProcesses": processes()})
    elif command[:3] == ["device", "process", "terminate"]:
        pid = int(option(args, "--pid"))
        set_processes([p for p in processes() if p["processIdentifier"] != pid])
        write_result(args, {})
    elif command[:3] == ["device", "process", "launch"]:
        launch(args)
    else:
        write_result(args, error=f"fake devicectl: unsupported command {args}")
        sys.exit(1)


def launch(args):
    bundle_id_at = args.index("--json-output") + 2
    bundle_id, arguments = args[bundle_id_at], args[bundle_id_at + 1:]
    app_name = "HelloZ.app"
    set_processes([p for p in processes() if p["processIdentifier"] != PID] + [
        {"executable": installed_url(app_name) + "HelloZ", "processIdentifier": PID}])

    def terminated(signum, _frame):
        set_processes([p for p in processes() if p["processIdentifier"] != PID])
        print(f"App terminated due to signal {signum}.", flush=True)
        write_result(args, {"process": {"processIdentifier": PID}, "terminationResult": {"terminatingSignal": signum}})
        sys.exit(0)

    signal.signal(signal.SIGTERM, terminated)
    print(f"Launched application with {bundle_id} bundle identifier.", flush=True)
    print("Waiting for the application to terminate…", flush=True)
    print("HelloZ launched", flush=True)
    mode = os.environ.get("DEVICECTL_CHILD_HELLOZ_MODE", "")
    print(f"arguments: {' '.join(arguments)} HELLOZ_MODE={mode}", flush=True)
    while True:
        time.sleep(0.2)
        if not any(p["processIdentifier"] == PID for p in processes()):
            # `device process terminate` ended the app.
            print("App terminated due to signal 15.", flush=True)
            write_result(args, {"process": {"processIdentifier": PID}, "terminationResult": {"terminatingSignal": 15}})
            return


if __name__ == "__main__":
    main()

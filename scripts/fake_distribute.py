#!/usr/bin/env python3
"""A stand-in for Distribute App's tools in the e2e tests: `xcodebuild
-exportArchive` (zcode runs it when ZCODE_EXPORT_ARCHIVE names it),
`notarytool` (ZCODE_NOTARYTOOL) and `stapler` (ZCODE_STAPLER), told apart
by their arguments. Nothing is ever sent to Apple: uploads and
notarization are pretended. State lives in $FAKE_DISTRIBUTE_DIR:

    calls.jsonl     every invocation: its tool, arguments and, for exports,
                    the ExportOptions.plist as a dictionary; for submissions
                    the names in the zip
    fail_upload     when present, an upload fails as App Store Connect
                    refusing it would
    stapled.txt     the apps `stapler staple` was given

An export copies the archived app into the export folder (what a real
`developer-id` or `mac-application` export leaves, minus the signing) and
prints xcodebuild's lines; an upload prints its progress and
`** EXPORT SUCCEEDED **`. `notarytool submit --keychain-profile <p>`
answers as Apple would: Accepted, Invalid for the profile `rejected`, and
no credentials for `missing`; `notarytool log` lists a rejection's issues.
"""

import json
import os
import plistlib
import shutil
import sys
import zipfile

STATE = os.environ["FAKE_DISTRIBUTE_DIR"]


def record(entry):
    os.makedirs(STATE, exist_ok=True)
    with open(os.path.join(STATE, "calls.jsonl"), "a") as f:
        f.write(json.dumps(entry) + "\n")


def value(args, flag):
    return args[args.index(flag) + 1] if flag in args else None


def export(args):
    archive, export_path, options_path = value(args, "-archivePath"), value(args, "-exportPath"), value(args, "-exportOptionsPlist")
    with open(options_path, "rb") as f:
        options = plistlib.load(f)
    record({"tool": "xcodebuild", "args": args, "options": options})
    with open(os.path.join(archive, "Info.plist"), "rb") as f:
        info = plistlib.load(f)
    name = info.get("Name", "App")
    print(f"note: Using the fake exporter for {name} ({options.get('method')})", flush=True)
    if options.get("destination") == "upload":
        if os.path.exists(os.path.join(STATE, "fail_upload")):
            print("error: exportArchive Upload failed: The bundle version must be higher than the previously uploaded version.", flush=True)
            print("** EXPORT FAILED **", flush=True)
            return 70
        for progress in ("Progress 25%: Preparing upload", "Progress 75%: Uploading package", "Progress 100%: Upload succeeded."):
            print(progress, flush=True)
        print(f"Uploaded {name}", flush=True)
        print("** EXPORT SUCCEEDED **", flush=True)
        return 0
    app_path = info["ApplicationProperties"]["ApplicationPath"]
    app = os.path.join(archive, "Products", app_path)
    os.makedirs(export_path, exist_ok=True)
    target = os.path.join(export_path, os.path.basename(app))
    shutil.rmtree(target, ignore_errors=True)
    shutil.copytree(app, target, symlinks=True)
    with open(os.path.join(export_path, "ExportOptions.plist"), "wb") as f:
        plistlib.dump(options, f)
    print(f"Exported {name} to: {export_path}", flush=True)
    print("** EXPORT SUCCEEDED **", flush=True)
    return 0


def notarytool(args):
    profile = value(args, "--keychain-profile")
    if args[0] == "submit":
        path = args[1]
        names = zipfile.ZipFile(path).namelist() if zipfile.is_zipfile(path) else []
        record({"tool": "notarytool", "args": args, "zip": names})
        if profile == "missing":
            print(f"Error: No Keychain password item found for profile: {profile}", file=sys.stderr, flush=True)
            return 69
        print(f"Conducting pre-submission checks for {os.path.basename(path)} and initiating connection to the Apple notary service...", flush=True)
        status = "Invalid" if profile == "rejected" else "Accepted"
        print(json.dumps({"id": "2efe2717-52ef-43a5-96dc-0797e4ca1041", "status": status, "message": "Processing complete"}), flush=True)
        return 0
    if args[0] == "log":
        record({"tool": "notarytool", "args": args})
        print(json.dumps({
            "jobId": args[1],
            "status": "Invalid",
            "issues": [{
                "severity": "error",
                "code": None,
                "path": "HelloClassic.zip/HelloClassic.app/Contents/MacOS/HelloClassic",
                "message": "The binary is not signed with a valid Developer ID certificate.",
                "architecture": "arm64",
            }],
        }, indent=2), flush=True)
        return 0
    print(f"Error: unknown command {args[0]}", file=sys.stderr)
    return 64


def stapler(args):
    record({"tool": "stapler", "args": args})
    with open(os.path.join(STATE, "stapled.txt"), "a") as f:
        f.write(args[1] + "\n")
    print("Processing: " + args[1], flush=True)
    print("The staple and validate action worked!", flush=True)
    return 0


def main():
    args = sys.argv[1:]
    if "-exportArchive" in args:
        return export(args)
    if args[:1] in (["submit"], ["log"]):
        return notarytool(args)
    if args[:1] == ["staple"]:
        return stapler(args)
    print(f"fake_distribute.py: unexpected arguments {args}", file=sys.stderr)
    return 64


if __name__ == "__main__":
    sys.exit(main())

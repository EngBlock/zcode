#!/usr/bin/env python3
"""Packages zcode as a Mac app: the bundle, its signature, notarization, the
archives it ships as, and the update feed.

    cargo build --release
    python3 scripts/package.py bundle                  # target/release/bundle/zcode.app
    python3 scripts/package.py sign [--adhoc]          # Developer ID (ZCODE_SIGN_IDENTITY), or ad hoc
    python3 scripts/package.py notarize                # notarytool with a keychain profile, then stapler
    python3 scripts/package.py zip | dmg               # zcode-<version>.zip / .dmg next to the app
    python3 scripts/package.py appcast --url <base>    # appcast.json for the in-app update check
    python3 scripts/package.py release --url <base>    # all of the above, in order

Every subcommand takes `--out <dir>` (the bundle's folder; default
target/release/bundle), `--dry-run` (print the commands, run nothing that
signs, uploads or writes outside the output folder) and `--quiet`.

Credentials never live here: signing uses an identity in the keychain
(`--identity`, or ZCODE_SIGN_IDENTITY, default "Developer ID Application"),
and notarization a keychain profile (`--profile`, or ZCODE_NOTARY_PROFILE)
stored once with `xcrun notarytool store-credentials <name>`.
"""

import argparse
import hashlib
import json
import os
import plistlib
import re
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HERE = os.path.join(ROOT, "scripts", "bundle")
IDENTIFIER = "dev.zcode.zcode"
NAME = "zcode"
MINIMUM_SYSTEM = "14.0"


def version_from_cargo(path=os.path.join(ROOT, "Cargo.toml")):
    """The `[package]` version of Cargo.toml (the binary's `--version`)."""
    in_package = False
    with open(path) as f:
        for line in f:
            if line.startswith("["):
                in_package = line.strip() == "[package]"
            elif in_package:
                m = re.match(r'\s*version\s*=\s*"([^"]+)"', line)
                if m:
                    return m.group(1)
    raise SystemExit("no [package] version in " + path)


def info_plist(version, build=None):
    """The bundle's Info.plist. Document types are what Xcode claims for
    projects, workspaces and Swift sources (Alternate: Xcode stays the
    default opener), plus folders, so Finder's Open With lists zcode."""
    document_types = [
        {"CFBundleTypeName": "Xcode Project", "CFBundleTypeRole": "Editor", "LSHandlerRank": "Alternate",
         "LSItemContentTypes": ["com.apple.xcode.project"]},
        {"CFBundleTypeName": "Xcode Workspace", "CFBundleTypeRole": "Editor", "LSHandlerRank": "Alternate",
         "LSItemContentTypes": ["com.apple.dt.document.workspace"]},
        {"CFBundleTypeName": "Swift Source", "CFBundleTypeRole": "Editor", "LSHandlerRank": "Alternate",
         "LSItemContentTypes": ["public.swift-source"]},
        {"CFBundleTypeName": "Source Code", "CFBundleTypeRole": "Editor", "LSHandlerRank": "Alternate",
         "LSItemContentTypes": ["public.source-code", "public.plain-text", "public.json", "public.yaml",
                                "com.apple.property-list", "net.daringfireball.markdown"]},
        {"CFBundleTypeName": "Folder", "CFBundleTypeRole": "Viewer", "LSHandlerRank": "Alternate",
         "LSItemContentTypes": ["public.folder"]},
    ]
    # Xcode exports these two; declaring them imported keeps the association
    # working on a Mac without Xcode.
    imported_types = [
        {"UTTypeIdentifier": "com.apple.xcode.project", "UTTypeDescription": "Xcode Project",
         "UTTypeConformsTo": ["com.apple.package", "public.composite-content"],
         "UTTypeTagSpecification": {"public.filename-extension": ["xcodeproj"]}},
        {"UTTypeIdentifier": "com.apple.dt.document.workspace", "UTTypeDescription": "Xcode Workspace",
         "UTTypeConformsTo": ["com.apple.package", "public.composite-content"],
         "UTTypeTagSpecification": {"public.filename-extension": ["xcworkspace"]}},
    ]
    return {
        "CFBundleDevelopmentRegion": "en",
        "CFBundleDisplayName": NAME,
        "CFBundleDocumentTypes": document_types,
        "CFBundleExecutable": NAME,
        "CFBundleIconFile": NAME,
        "CFBundleIdentifier": IDENTIFIER,
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": NAME,
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": build or version,
        "LSApplicationCategoryType": "public.app-category.developer-tools",
        "LSMinimumSystemVersion": MINIMUM_SYSTEM,
        "NSAppleEventsUsageDescription": "zcode drives Xcode's canvas through System Events for Show in Xcode Canvas.",
        "NSHighResolutionCapable": True,
        "NSHumanReadableCopyright": "Copyright © 2026 Nathan Beddoe. MIT License.",
        "NSSupportsAutomaticGraphicsSwitching": True,
        "UTImportedTypeDeclarations": imported_types,
    }


class Packager:
    def __init__(self, args):
        self.args = args
        self.out = os.path.abspath(args.out)
        self.app = os.path.join(self.out, NAME + ".app")
        self.version = version_from_cargo()

    def say(self, text):
        if not self.args.quiet:
            print(text)

    def run(self, command, dry=False, **kwargs):
        """Runs a command; with --dry-run, prints the ones that sign, upload
        or write outside the output folder (`dry=True`) instead."""
        if dry and self.args.dry_run:
            self.say("  (dry run) " + " ".join(command))
            return subprocess.CompletedProcess(command, 0, "", "")
        self.say("  " + " ".join(command))
        return subprocess.run(command, check=True, text=True, **kwargs)

    # -- bundle --------------------------------------------------------------

    def icon(self):
        """The icon, drawn by scripts/bundle/make_icon.swift (kept in the
        output folder until the script changes)."""
        script = os.path.join(HERE, "make_icon.swift")
        icns = os.path.join(self.out, NAME + ".icns")
        if not os.path.exists(icns) or os.path.getmtime(icns) < os.path.getmtime(script):
            os.makedirs(self.out, exist_ok=True)
            self.run(["swift", script, icns])
        return icns

    def bundle(self):
        binary = os.path.abspath(self.args.binary)
        if not os.path.exists(binary):
            raise SystemExit(f"{binary} is missing: cargo build --release first")
        self.say(f"Bundling {NAME} {self.version} into {self.app}")
        icns = self.icon()
        if os.path.exists(self.app):
            shutil.rmtree(self.app)
        contents = os.path.join(self.app, "Contents")
        for folder in ("MacOS", "Resources", os.path.join("Library", "LaunchAgents")):
            os.makedirs(os.path.join(contents, folder))
        shutil.copy2(binary, os.path.join(contents, "MacOS", NAME))
        shutil.copy2(icns, os.path.join(contents, "Resources", NAME + ".icns"))
        with open(os.path.join(contents, "Info.plist"), "wb") as f:
            plistlib.dump(info_plist(self.version, self.args.build), f, sort_keys=True)
        with open(os.path.join(contents, "PkgInfo"), "w") as f:
            f.write("APPL????")
        # The login item's agent (zcode ▸ Open at Login): the bundle's own
        # binary with --background, registered through SMAppService.
        shutil.copy2(os.path.join(HERE, "background.plist"),
                     os.path.join(contents, "Library", "LaunchAgents", IDENTIFIER + ".background.plist"))
        self.say(f"  {self.app}")
        return self.app

    # -- sign and notarize ---------------------------------------------------

    def sign(self):
        identity = "-" if self.args.adhoc else (self.args.identity or os.environ.get("ZCODE_SIGN_IDENTITY") or "Developer ID Application")
        self.say(f"Signing {self.app} as {'ad hoc' if identity == '-' else identity!r}")
        command = ["codesign", "--force", "--options", "runtime", "--timestamp", "--sign", identity,
                   "--entitlements", os.path.join(HERE, "zcode.entitlements"), "--identifier", IDENTIFIER, self.app]
        if identity == "-":
            # Ad hoc signatures can't be timestamped.
            command.remove("--timestamp")
        self.run(command, dry=True)
        if not self.args.dry_run:
            self.run(["codesign", "--verify", "--deep", "--strict", "--verbose=2", self.app])

    def notarize(self):
        profile = self.args.profile or os.environ.get("ZCODE_NOTARY_PROFILE")
        if not profile and not self.args.dry_run:
            raise SystemExit("notarize needs --profile or ZCODE_NOTARY_PROFILE (xcrun notarytool store-credentials <name>)")
        upload = os.path.join(self.out, f"{NAME}-notarize.zip")
        self.say(f"Notarizing {self.app} with the keychain profile {profile or '<profile>'!r}")
        self.run(["ditto", "-c", "-k", "--keepParent", self.app, upload])
        self.run(["xcrun", "notarytool", "submit", upload, "--keychain-profile", profile or "<profile>", "--wait"], dry=True)
        self.run(["xcrun", "stapler", "staple", self.app], dry=True)
        self.run(["spctl", "--assess", "--type", "execute", "--verbose=2", self.app], dry=True)
        os.remove(upload)

    # -- archives and the feed -----------------------------------------------

    def zip(self):
        """The archive the in-app update downloads (`ditto`, as Apple's
        notarization guide packs apps, so symlinks and signatures keep)."""
        out = os.path.join(self.out, f"{NAME}-{self.version}.zip")
        if os.path.exists(out):
            os.remove(out)
        self.say(f"Archiving {out}")
        self.run(["ditto", "-c", "-k", "--keepParent", self.app, out])
        return out

    def dmg(self):
        out = os.path.join(self.out, f"{NAME}-{self.version}.dmg")
        if os.path.exists(out):
            os.remove(out)
        staging = os.path.join(self.out, "dmg")
        shutil.rmtree(staging, ignore_errors=True)
        os.makedirs(staging)
        self.say(f"Making {out}")
        self.run(["ditto", self.app, os.path.join(staging, NAME + ".app")])
        os.symlink("/Applications", os.path.join(staging, "Applications"))
        self.run(["hdiutil", "create", "-quiet", "-volname", NAME, "-srcfolder", staging, "-ov", "-format", "UDZO", out])
        shutil.rmtree(staging)
        return out

    def appcast(self):
        """appcast.json: what `zcode ▸ Check for Updates…` reads (src/updates.rs).
        `--url` is where the zip will be served from (its folder, or the
        full URL)."""
        archive = os.path.join(self.out, f"{NAME}-{self.version}.zip")
        if not os.path.exists(archive):
            self.zip()
        url = self.args.url or ""
        if not url.endswith(".zip"):
            url = url.rstrip("/") + "/" + os.path.basename(archive)
        feed = {
            "version": self.version,
            "url": url,
            "sha256": sha256(archive),
            "length": os.path.getsize(archive),
            "minimum_system": MINIMUM_SYSTEM,
            "notes": self.args.notes or "",
        }
        out = os.path.join(self.out, "appcast.json")
        with open(out, "w") as f:
            json.dump(feed, f, indent=2)
            f.write("\n")
        self.say(f"Wrote {out}")
        return out

    def release(self):
        self.bundle()
        self.sign()
        self.notarize()
        self.zip()
        self.dmg()
        self.appcast()


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=["bundle", "icon", "sign", "notarize", "zip", "dmg", "appcast", "release"])
    parser.add_argument("--out", default=os.path.join(ROOT, "target", "release", "bundle"), help="the bundle's folder")
    parser.add_argument("--binary", default=os.path.join(ROOT, "target", "release", NAME), help="the release binary")
    parser.add_argument("--build", help="CFBundleVersion (default: the version)")
    parser.add_argument("--identity", help="the signing identity's name (ZCODE_SIGN_IDENTITY)")
    parser.add_argument("--adhoc", action="store_true", help="sign ad hoc, for local testing")
    parser.add_argument("--profile", help="notarytool's keychain profile (ZCODE_NOTARY_PROFILE)")
    parser.add_argument("--url", help="where the zip is served from, for appcast.json")
    parser.add_argument("--notes", help="release notes for appcast.json")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()
    packager = Packager(args)
    if args.command == "icon":
        print(packager.icon())
        return
    getattr(packager, args.command)()


if __name__ == "__main__":
    main()

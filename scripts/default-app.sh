#!/usr/bin/env bash
# Makes zcode the app that opens Xcode files (.xcodeproj, .xcworkspace, .swift),
# puts Xcode back, or prints what opens each one today:
#
#   python3 scripts/package.py bundle      # the bundle the association points at
#   scripts/default-app.sh                 # zcode opens them from Finder
#   scripts/default-app.sh --restore       # Xcode does again
#   scripts/default-app.sh --print         # what Launch Services says now
#
# zcode's Info.plist claims these types as Alternate, so Finder's Open With
# lists zcode while Xcode stays the default; this asks Launch Services to make
# zcode the default instead, through NSWorkspace.setDefaultApplication, which
# wants the type as a UTI (an extension is silently ignored on macOS 15+) and,
# on macOS 26.4 and later, a confirmation the first time a type is changed.
# No sudo: the setting is the user's.
set -euo pipefail

invoked_from="$PWD"
cd "$(dirname "$0")/.."

# Xcode's types for projects (which also cover .xcode) and workspaces, and its
# Swift source type. zcode imports the first two, so they exist without Xcode.
TYPES=(com.apple.xcode.project com.apple.dt.document.workspace public.swift-source)
XCODE="com.apple.dt.Xcode"
LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"

usage() { awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"; }

# `canonical <path>` for a symlink-free absolute path; needs the path to exist.
canonical() { (cd "$(dirname "$1")" 2>/dev/null && printf '%s/%s' "$(pwd)" "$(basename "$1")"); }

mode=set
app=""
while [ $# -gt 0 ]; do
  case "$1" in
    --print) mode=print ;;
    --restore) app="$XCODE" ;;
    --app)
      shift
      [ $# -gt 0 ] || { echo "--app needs the bundle's path" >&2; exit 2; }
      case "$1" in /*) app="$1" ;; *) app="$invoked_from/$1" ;; esac
      app="$(canonical "$app" || printf '%s' "$app")"
      ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done

# Which bundle to point at: whatever was asked for, else the installed app,
# else the one this checkout just built.
if [ -z "$app" ]; then
  for candidate in "${ZCODE_APP:-}" "/Applications/zcode.app" "$HOME/Applications/zcode.app" \
                   "target/release/bundle/zcode.app"; do
    if [ -n "$candidate" ] && [ -d "$candidate" ]; then app="$(canonical "$candidate")"; break; fi
  done
fi

if [ "$mode" = set ] && [ "$app" != "$XCODE" ]; then
  if [ -z "$app" ]; then
    echo "no zcode bundle found — run: python3 scripts/package.py bundle" >&2
    echo "(or say where it is: scripts/default-app.sh --app /Applications/zcode.app)" >&2
    exit 1
  fi
  if [ ! -d "$app" ]; then
    echo "no app bundle at $app" >&2
    exit 1
  fi
  "$LSREGISTER" -f "$app"
fi

# The Launch Services work is a dozen lines of Swift; keep it in this file and
# run it from a here-doc (zcode already needs Xcode's toolchain anyway).
swift_file="$(mktemp -t zcode-default-app)"
trap 'rm -f "$swift_file"' EXIT
cat > "$swift_file" <<'SWIFT'
import AppKit
import Foundation
import UniformTypeIdentifiers

let args = Array(CommandLine.arguments.dropFirst())
let mode = args[0], requested = args[1], types = Array(args.dropFirst(2))
let width = types.map(\.count).max() ?? 0

// The app a Finder double-click opens, as a URL.
func target() -> URL? {
    requested.contains("/") ? URL(fileURLWithPath: requested)
                            : NSWorkspace.shared.urlForApplication(withBundleIdentifier: requested)
}

/// Why a request didn't take: the user kept the old app, or the error itself.
func reason(_ error: Error?) -> String {
    guard let error = error as NSError? else { return "unchanged" }
    // -128 (userCanceledErr) is what the confirmation dialog's "Keep …" comes
    // back as; NSUserCancelledError is the same answer on other paths.
    let canceled = -128
    let underlying = error.userInfo[NSUnderlyingErrorKey] as? NSError
    if error.code == canceled || error.code == NSUserCancelledError || underlying?.code == canceled {
        return "declined"
    }
    return error.localizedDescription
}

/// The app that opens a type now, and its bundle ID.
func opener(_ uti: String) -> (name: String, id: String?) {
    guard let type = UTType(uti), let url = NSWorkspace.shared.urlForApplication(toOpen: type) else {
        return ("-", nil)
    }
    var name = FileManager.default.displayName(atPath: url.path)
    if name.hasSuffix(".app") { name.removeLast(4) }
    return (name, Bundle(url: url)?.bundleIdentifier)
}

// A bundle ID (--restore) or a path (--app, the built bundle). Printing needs
// no bundle at all, so only setting insists on one.
let appURL = requested.isEmpty ? nil : target()
if mode == "set", appURL == nil {
    FileHandle.standardError.write(Data("not an installed app: \(requested)\n".utf8))
    exit(1)
}
let wanted = appURL.flatMap { Bundle(url: $0)?.bundleIdentifier }

var failed = false
for uti in types {
    let pad = String(repeating: " ", count: max(0, width - uti.count))
    guard let type = UTType(uti) else {
        print("\(uti)\(pad)  unknown content type")
        failed = true
        continue
    }
    let before = opener(uti)
    if mode == "print" {
        print("\(uti)\(pad)  \(before.name)")
        continue
    }
    guard let app = appURL else { continue }
    var answer = "the dialog was never answered"
    let done = DispatchSemaphore(value: 0)
    NSWorkspace.shared.setDefaultApplication(at: app, toOpen: type) { error in
        answer = reason(error)
        done.signal()
    }
    _ = done.wait(timeout: .now() + 600)  // a confirmation may sit unanswered
    // The callback says nothing about consent, and Launch Services writes the
    // change in its own time; what opens the type now is the only truth.
    var after = opener(uti)
    for _ in 0..<20 where after.id != wanted {
        usleep(300_000)
        after = opener(uti)
    }
    if after.id != nil, after.id == wanted {
        print("\(uti)\(pad)  open in \(after.name)\(after.name == before.name ? "" : " (was \(before.name))")")
    } else {
        print("\(uti)\(pad)  still \(after.name) (\(answer))")
        failed = true
    }
}
exit(failed ? 1 : 0)
SWIFT

label="${app##*/}"; label="${label%.app}"
[ "$app" = "$XCODE" ] && label="Xcode"

if [ "$mode" = print ]; then
  swift "$swift_file" print "$app" "${TYPES[@]}"
else
  echo "asking macOS to open these with $label"
  echo "(macOS 26.4 and later ask for confirmation the first time)"
  swift "$swift_file" set "$app" "${TYPES[@]}"
fi

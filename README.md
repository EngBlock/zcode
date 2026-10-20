# zcode

A fast, Zed-inspired editor for Apple-platform development, written in Rust on
[GPUI](https://gpui.rs) (Zed's GPU UI framework) via
[GPUI Kit](https://gpui-kit.com). It edits, builds, runs, tests and debugs
Xcode projects and Swift packages without opening Xcode. It uses the
toolchain Xcode installs: `xcodebuild`, `simctl`, `sourcekit-lsp` and
`lldb-dap`. The web app an Apple app ships with (a Vite app in `web/`, the
pages of a web view) is edited in the same window, with the web's language
servers and Emmet, and run there: its `package.json` scripts are in the
scheme menu beside the app's schemes, and Run starts its dev server next
to the running app.

```
cargo build --release
./target/release/zcode path/to/App.xcodeproj   # or .xcworkspace, Package.swift, a folder, a file
```

Requirements: macOS with Xcode installed (`xcode-select -p` pointing at it), Rust 1.90+.

As a Mac app:

```
python3 scripts/package.py bundle          # target/release/bundle/zcode.app, from the release build
python3 scripts/package.py sign --adhoc    # or sign (Developer ID, ZCODE_SIGN_IDENTITY) and notarize (ZCODE_NOTARY_PROFILE)
open target/release/bundle/zcode.app
```

Then zcode ▸ Install CLI puts a `zcode` command in `/usr/local/bin`, zcode ▸
Open at Login starts the resident instance (`--background`) when you log in,
zcode ▸ Check for Updates… installs a newer release, and Finder's Open With
lists zcode for `.xcodeproj`, `.xcworkspace`, Swift sources and folders.
`scripts/package.py release` does bundle, sign, notarize, zip, dmg and the
update feed in one go (`--dry-run` prints what it would run).

In that list zcode comes second until you say otherwise, since the bundle
claims Xcode's types as Alternate. To make a double-click open them in zcode
instead, for Xcode projects (`.xcodeproj`, `.xcode`), workspaces and Swift
sources:

```
scripts/default-app.sh            # zcode opens them from Finder
scripts/default-app.sh --restore  # Xcode does again
scripts/default-app.sh --print    # what opens each one now
```

It points at `/Applications/zcode.app` when that exists, otherwise at this
checkout's `target/release/bundle/zcode.app` (`--app` says where). The setting
is per user, so no `sudo`, and macOS 26.4 and later asks you to confirm the
first time it changes a type (a type it has been told about before goes
through silently); older systems apply them at once.

Releases are made by tag: bump the version in `Cargo.toml`, commit, and push
`v<version>` (`git tag v0.2.0 && git push origin v0.2.0`).
[`.github/workflows/release.yml`](.github/workflows/release.yml) builds,
signs with the Developer ID, notarizes and publishes the zip, the dmg and
`appcast.json` as a GitHub release; the in-app update reads
`releases/latest/download/appcast.json`, so a tag with a hyphen
(`v0.3.0-beta.1`) is a prerelease that no one is offered. The secrets it
needs are listed at the top of the workflow.

## Documentation

* [Features](docs/features.md): everything zcode does, area by area, with
  its Xcode-compatible keys: projects, property lists, asset catalogs,
  SwiftUI previews, Swift packages, editor panes, terminal, coding
  assistant, predictive completion, editing, Vim mode, language
  intelligence, web apps, navigation, source control, build and run,
  testing, debugging.
* [zcode as an MCP server](docs/mcp.md): `zcode mcp` for Claude Code, Codex,
  Gemini CLI, Cursor and other MCP clients.
* [Performance](docs/performance.md): launch times against Xcode and AppKit,
  memory, and how zcode stays fast.
* [How it fits together](docs/architecture.md): the source layout, the
  editor crate and the dependencies.
* [Testing](docs/testing.md): unit tests, the end-to-end suite and the
  benchmarks.
* [Known limitations](docs/limitations.md)
* [Xcode parity](docs/parity.md) and the [Xcode 27 inventory](docs/xcode-inventory.md)
  it was matched against; the [roadmap](docs/roadmap.md).
* Design notes: [the editor engine](docs/editor-engine.md),
  [scale benchmarks](docs/scale.md), [SwiftUI previews](docs/previews.md).

## License

MIT ([`LICENSE`](LICENSE)), copyright Nathan Beddoe.

The exceptions are code derived from other projects, which keeps its
license: `crates/zcode-editor` is Apache-2.0, derived from GPUI Component
(Longbridge), and the patched crates in `vendor/` are GPUI Component and
GPUI (Zed Industries), also Apache-2.0. See their `LICENSE-APACHE`,
`NOTICE` and `ZCODE-PATCH.md` files.

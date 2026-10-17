# SwiftUI previews

Roadmap item 4.4. zcode ships a bridge to Xcode's canvas. This file
describes the bridge, how Xcode's canvas works, and how far a native
preview can go without private frameworks, measured on this Mac. A native
preview is not shipped yet: the experiment works for an app target, but
it isn't robust enough for arbitrary projects (see "What shipping it
would take").

## What ships: Show in Xcode Canvas

**Finding previews.** `src/previews.rs` finds `#Preview` macros and
`PreviewProvider` conformances in Swift source with tree-sitter, the same
way test diamonds are found (`src/testing/discover.rs`, whose parser it
shares). That covers every `#Preview` form: named, with traits,
`arguments:`, a widget's `timeline:`, and a UIKit view controller. It also
covers declarations under `#if DEBUG`, providers declared as extensions,
and nested types. A file is parsed when it opens and again 300 ms after
typing stops, in the background, from the buffer's text (unsaved edits
included).

**The gutter.** Each preview gets a mark over its line number, where test
diamonds go: a small canvas with a play triangle, filled while hovered.
The mark is on the `#Preview` line, or on the provider's `struct` line,
below its attributes. It's a lane of the editor's gutter
(`crates/zcode-editor/src/gutter.rs`), so it follows scrolling, folding,
soft wrap and every pane's view of the file. Clicking a mark shows that
preview in Xcode.

**The command.** Editor ▸ Show in Xcode Canvas, ⌥⌘↩ (Xcode 27's key for
Editor ▸ Canvas, from `IDEKit.xcplugindata`: `ShowPreviewEditor`, titled
"Canvas" in the source editor). It shows the preview the cursor is in, or
opens the file at the cursor's line when the cursor isn't in one (the
canvas shows all of a file's previews). In a file without previews it
says so and does nothing. The file is saved first, so Xcode builds what
the editor shows. Xcode reloads the file after each later save in zcode,
and with Automatically Refresh Canvas the canvas follows.

**The handoff** (`src/xcode_app.rs`, on a background thread):

1. `xed <project>` opens the workspace, project or package folder in the
   Xcode that `xcode-select` points at (`DEVELOPER_DIR` overrides it, as
   for every Xcode tool).
2. System Events waits up to a minute for the project's window, which is
   titled `HelloZ — …` or shows `HelloZ.xcodeproj`. A new window gets a
   further second to load the project's files. This step matters: on a
   cold launch, a file that reached Xcode before its project had loaded
   opened in a window of its own, where the canvas reported "Cannot use
   previews in this file — Active scheme does not build this file".
3. `xed --project <project> --line <n> <file>` opens the file in that
   window with line `n` selected.
4. Xcode has no scripting command for the canvas, so System Events waits
   for the file to be in Xcode's front window (by its `AXDocument`, or
   the window's title, since Xcode reports `/tmp` where zcode has
   `/private/tmp`). Then it chooses Editor ▸ Canvas, unless that item is
   already checked: Xcode remembers the canvas per editor, and the item
   toggles. The Editor menu has two items named "Canvas": the toggle and
   the canvas's options submenu. If the canvas then shows "Canvas paused"
   (a project's first preview waits to be asked for), the script chooses
   Editor ▸ Canvas ▸ Refresh Canvas (⌥⌘P).

Steps 2 and 4 need zcode to be allowed to control the computer (System
Settings ▸ Privacy & Security ▸ Accessibility), and macOS asks once for
permission to send System Events Apple events. Without either
permission, the file still opens at the line, and a notification says to
press ⌥⌘↩ in Xcode. The control channel's `dump` has the last request and
its outcome under `previews.last`: `shown`, `already shown`, `opened
without the canvas: …`, `failed: …` or `recorded`.

`ZCODE_XCODE_COMMANDS=<file>` records each command's arguments as a JSON
line instead of running it. The e2e suite sets it for every launch, so
nothing it does opens Xcode, and its checks compare the recorded
commands. Open in Xcode (project settings, the project navigator's
context menu) goes through the same module and the same switch.

**Checked against the real Xcode** (Xcode 27.0, 27A5252f, in
`/Applications`), by hand, from zcode's release build on a copy of
HelloZ given a file with a named `#Preview` and a `PreviewProvider`:

- From a cold Xcode launch, a mark's click (and ⌥⌘↩) took 8–19 s to
  open the project, then the file in the project's window at the line,
  and to report `already shown`: Xcode had remembered the canvas for
  that file. The `shown` path was checked by running the canvas step
  with the canvas turned off.
- The canvas rendered both previews as tabs ("Light" and the provider's
  "Gallery"). It stayed on "Light" when the provider's line was the one
  selected, so the line places the cursor and doesn't pick the tab.
- "Canvas paused" was found and refreshed in about 2 s.

The Xcode `xcode-select` points at on this Mac (the 27.0 beta in
`~/Downloads`) had never been opened, and stopped at its first-launch
"Download & Install" platforms dialog. zcode can't get past that dialog,
so Show in Xcode Canvas then reports that Xcode didn't show the file in
time.

## How Xcode's canvas works

Everything below was read from Xcode 27's bundle and a Debug build of
HelloZ. None of it is documented API.

- **Build products.** A Debug build (`ENABLE_DEBUG_DYLIB = YES`,
  `ENABLE_PREVIEWS = YES`, Xcode's defaults) splits the app into a stub
  executable, `HelloZ.debug.dylib` holding all of the app's code, and
  `__preview.dylib`, which links the debug dylib. The canvas runs that
  code in `XCPreviewAgent.app` (in each simulator platform's
  `Developer/Library/Xcode/Agents`), with `libPreviewsJITStubExecutor`.
  Editor ▸ Canvas ▸ Use Legacy Previews Execution switches back from that
  JIT mode. Xcode's side lives in private frameworks (`PreviewsPipeline`,
  `PreviewsMessagingHost`, `PreviewsFoundationHost`, `PreviewsXcode` and
  more in `SharedFrameworks`).
- **What `#Preview` expands to.** The macro (`PreviewsMacros`, declared in
  SwiftUI's interface) becomes a type conforming to
  `DeveloperToolsSupport.PreviewRegistry`, with `fileID`, `line`,
  `column` and `makePreview() throws -> Preview`. The debug dylib exports
  it (with testability on), for example
  `HelloZ.$s6HelloZ0022ContentViewswift_…PreviewRegistryfMu_`.
- **`Preview` is opaque.** It has no public members. `Mirror` shows its
  fields: `displayName`, `dataSource: DataSource` and `traits`. Inside
  `dataSource` is a `DefaultPreviewSource<ViewPreviewBody>` with
  `contentDomain`, `contentCategory`, `supportsOnDevicePreviews`,
  `supportsInteractivePreviews` and `structure`. A body closure is
  probably reachable that way, but only through private layouts, which
  have changed between Xcode releases. zcode has the source and knows
  each preview's range, so it can take the body's text instead.
- **Literal edits.** `DeveloperToolsSupport` publicly declares
  `__designTimeString/Integer/Float/Boolean(_:fallback:)` and
  `__designTimeApplyIncrementalValues(_:)`. These are the hooks behind
  the canvas's instant literal updates, but the thunks that call them,
  and the channel that feeds them values, are Xcode's.

## How far a native preview goes: measured

**Setup.** HelloZ; the Xcode 27.0 beta's command line tools (27A5228h);
an iOS 27 simulator device created for the experiment and deleted after
it; this MacBook while other sessions ran their own builds and e2e
suites.

**The idea: the app is the preview host.**

- The app is built as usual. It's launched on the simulator with
  `SIMCTL_CHILD_DYLD_INSERT_LIBRARIES=<loader.dylib>`, a 50-line
  Objective-C loader (see the Appendix).
- The loader watches a folder on the Mac and `dlopen`s each new "thunk"
  dylib. It calls the thunk's `zcode_preview_make()` and makes the result
  the key window's root view controller. Then it draws the window to a
  PNG next to the dylib.
- A thunk is the preview's body, copied from the source into a
  `@ViewBuilder` function, together with any edited files (each given
  `@testable import HelloZ`). It's compiled against the app's module:
  `swiftc -emit-library -I <Products>`. Its symbols are resolved at load
  time from the already-loaded `HelloZ.debug.dylib`
  (`-undefined dynamic_lookup`), so nothing is loaded twice.

| Step | Time |
|---|---|
| Debug build of HelloZ for the simulator, from a clean DerivedData | 10.6 s |
| Thunk, first compile (module cache cold) | 3.0 s (2.3 s in Swift 6 mode) |
| Thunk, later edits to the preview's body | 0.32 s |
| Thunk with the edited view's file compiled in | 0.52 s |
| `dlopen` and the new root view shown | 3–12 ms |
| The window drawn to PNG (`drawViewHierarchyInRect`) | ~95 ms |
| Edit → PNG on the Mac (compile, the loader's 100 ms poll, draw) | 0.53–0.59 s |

What worked:

- **Preview body edits**, for example wrapping the view and setting
  `.preferredColorScheme(.dark)`. The new preview replaced the old one in
  the running app.
- **Edits to the view itself.** The edited `ContentView.swift` was
  compiled into the thunk, with its `#Preview` expanded by the default
  macro plugin. Its `ContentView` shadows the app's, so the preview
  showed the new body (a new SF Symbol and text) while the rest of the
  app's code stayed loaded.
- **`@Previewable @State var count = 3`** in a copied body compiled and
  rendered. But as a local in a function, it isn't live state.
- **Dynamic replacement**, which Xcode 12–15's previews used. The app was
  rebuilt with `OTHER_SWIFT_FLAGS = -Xfrontend -enable-implicit-dynamic
  -Xfrontend -enable-private-imports` (4.8 s incremental). A thunk with
  `@_dynamicReplacement(for: body)` in an `extension ContentView`
  (`@_private(sourceFile: "ContentView.swift") import HelloZ`, so the
  replacement reaches the private `counter`) replaced the body in the
  running app. That worked even though the replacement's `some View` has
  a different concrete type. A second thunk replaced it again (the
  latest wins), compiled in 0.31–0.46 s. Unlike re-rooting, this updates
  the type everywhere the running app uses it, and keeps the app's state.

So there are two ways to apply an edit:

1. **Re-root (preview mode).** No special build flags. The preview
   becomes the window's root, and edited files are compiled into every
   thunk. Navigation state is gone, and only code reached from the
   preview's root is new.
2. **Dynamic replacement (hot reload).** This needs the app built with
   implicit dynamism (every declaration called through a replaceable
   thunk; Debug only). Replacements take effect everywhere and keep
   state. Changes to stored properties or type layout can't be replaced
   and need a rebuild. The attributes are underscored
   (`@_dynamicReplacement`, `@_private`): unofficial Swift that can
   change.

Neither needs a private framework: the simulator, `dlopen`, UIKit,
SwiftUI and the Swift compiler are all public.

## What shipping it would take

The experiment had one app target, one module and no packages. A robust
"Preview on Simulator" needs:

- **The target's exact compiler arguments**: module and framework search
  paths, Swift version and upcoming features, `-D` flags, package
  modules, macro plugins, C/Objective-C headers and the bridging header.
  The thunk above got by with the products folder. zcode already scrapes
  a compilation database from build logs for sourcekit-lsp, which would
  be the source, kept current per target.
- **Every preview form**:
  - traits (orientation, `sizeThatFitsLayout`, `fixedLayout`), applied
    by the host;
  - `arguments:` (new in Xcode 27);
  - UIKit `UIViewController` and `UIView` bodies (hosting chosen by the
    body's type, with multi-statement closures);
  - `@Previewable` lifted into a wrapper view's `@State`, as the macro
    does;
  - widget timelines (`#Preview(as: .systemSmall)`), which WidgetKit
    renders in an extension process an app can't host.
- **Libraries and Swift packages**, which have no app to host them. A
  small host app would be generated, built, ad hoc signed and installed
  per platform, linking the library's module as a dynamic library. More
  build plumbing; not tried here.
- **Mac apps.** There's no simulator, and `DYLD_INSERT_LIBRARIES` is
  ignored for hardened-runtime binaries (Xcode's Mac templates turn the
  hardened runtime on). The loader would have to be linked into Debug
  builds instead. Untested.
- **Lifecycle.** Swift images can't be unloaded, so each edit leaks its
  dylib (about 60 KB here) until the app relaunches. A crash in preview
  code ends the app, which then needs a relaunch and an error in zcode.
  The app's own launch work runs before the first preview.
- **Linking.** ld warns that `-undefined dynamic_lookup` is deprecated
  for the simulator. Linking against the debug dylib's install name and
  relying on dyld to match the loaded image is the alternative; untested.
- **Showing it in zcode.** The PNG gives a static canvas in a pane,
  about 0.55 s behind each edit. Interaction stays in Simulator.app:
  streaming its frames into zcode needs `SimulatorKit` (private), or
  screenshots, which are too slow to feel live.
- **Diagnostics.** The thunk compiler's errors would be mapped back to
  the file's lines (the wrapper shifts them) for the Issue navigator.
- **Never on a device.** That would mean signing every thunk, and devices
  don't take `DYLD_INSERT_LIBRARIES`.

A first version would be its own roadmap item:

1. Product ▸ Preview on Simulator, for app targets on iOS simulators, in
   re-root mode.
2. The build through the existing pipeline, then the app launched with a
   loader that zcode builds once per SDK with `clang` (about 1 s) and
   keeps in its caches.
3. Thunks compiled from the preview's tree-sitter range and the saved
   file, with the target's compile command.
4. The PNG in a canvas pane beside the editor, and errors in the Issue
   navigator.
5. e2e on HelloZ with the simulator.

Dynamic replacement could follow, behind a setting, since it changes how
Debug builds are compiled.

## Appendix: the experiment's code

The loader, built with `xcrun -sdk iphonesimulator clang -target
arm64-apple-ios18.0-simulator -fobjc-arc -dynamiclib -framework UIKit
loader.m -o loader.dylib`:

```objc
// Injected into the app on the simulator (SIMCTL_CHILD_DYLD_INSERT_LIBRARIES).
// Polls $ZCODE_PREVIEW_DIR for preview-<n>.dylib; loads each new one, shows
// what its zcode_preview_make() returns as the key window's root, and draws
// the window to preview-<n>.png.
#import <UIKit/UIKit.h>
#import <dlfcn.h>

static NSString *directory;
static NSInteger shown = -1;

static void check(void) {
    NSArray *names = [[NSFileManager defaultManager] contentsOfDirectoryAtPath:directory error:nil];
    NSInteger newest = -1;
    for (NSString *name in names) {
        if ([name hasPrefix:@"preview-"] && [name hasSuffix:@".dylib"]) {
            NSInteger n = [[name substringWithRange:NSMakeRange(8, name.length - 14)] integerValue];
            if (n > newest) newest = n;
        }
    }
    if (newest <= shown) return;
    shown = newest;
    NSString *path = [directory stringByAppendingPathComponent:[NSString stringWithFormat:@"preview-%ld.dylib", (long)newest]];
    void *handle = dlopen(path.fileSystemRepresentation, RTLD_NOW);
    if (!handle) { NSLog(@"zcode-preview: dlopen failed: %s", dlerror()); return; }
    void *(*make)(void) = dlsym(handle, "zcode_preview_make");
    if (!make) return;
    UIViewController *controller = (__bridge_transfer UIViewController *)make();
    UIWindow *window = nil;
    for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
        if ([scene isKindOfClass:UIWindowScene.class]) {
            for (UIWindow *w in ((UIWindowScene *)scene).windows) if (w.isKeyWindow || !window) window = w;
        }
    }
    window.rootViewController = controller;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 50 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
        UIGraphicsImageRenderer *renderer = [[UIGraphicsImageRenderer alloc] initWithBounds:window.bounds];
        NSData *png = [renderer PNGDataWithActions:^(UIGraphicsImageRendererContext *context) {
            [window drawViewHierarchyInRect:window.bounds afterScreenUpdates:YES];
        }];
        [png writeToFile:[[path stringByDeletingPathExtension] stringByAppendingPathExtension:@"png"] atomically:YES];
    });
}

__attribute__((constructor)) static void start(void) {
    const char *dir = getenv("ZCODE_PREVIEW_DIR");
    if (!dir) return;
    directory = [NSString stringWithUTF8String:dir];
    dispatch_async(dispatch_get_main_queue(), ^{
        [NSTimer scheduledTimerWithTimeInterval:0.1 repeats:YES block:^(NSTimer *timer) { check(); }];
    });
}
```

A thunk. `<body>` is the `#Preview`'s closure body, copied from the
source:

```swift
import SwiftUI
@testable import HelloZ

@MainActor enum ZcodePreview7 {
    @ViewBuilder static func make() -> some View {
        <body>
    }
}

@_cdecl("zcode_preview_make")
public func zcodePreviewMake() -> UnsafeMutableRawPointer {
    let bits = MainActor.assumeIsolated {
        Int(bitPattern: Unmanaged.passRetained(UIHostingController(rootView: AnyView(ZcodePreview7.make()))).toOpaque())
    }
    return UnsafeMutableRawPointer(bitPattern: bits)!
}
```

```
xcrun -sdk iphonesimulator swiftc -emit-library -module-name ZcodePreview7 \
  -target arm64-apple-ios18.0-simulator -sdk "$(xcrun --sdk iphonesimulator --show-sdk-path)" \
  -Onone -I <DerivedData>/Build/Products/Debug-iphonesimulator -F <same> \
  -Xlinker -undefined -Xlinker dynamic_lookup \
  -o thunks/preview-7.dylib thunk.swift [edited files, each with @testable import HelloZ]
mv thunks/preview-7.dylib previews/    # appears complete
```

The launch:

```
xcrun simctl install <udid> HelloZ.app
SIMCTL_CHILD_DYLD_INSERT_LIBRARIES=$PWD/loader.dylib SIMCTL_CHILD_ZCODE_PREVIEW_DIR=$PWD/previews \
  xcrun simctl launch --terminate-running-process <udid> dev.zcode.HelloZ
```

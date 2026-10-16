// A minimal native AppKit app: one window with a line of text. scripts/e2e.py
// builds it with `swiftc -O` and compares zcode's cold launch against it.
// Prints JSON timings measured like `zcode --startup-bench`, then quits:
// process start to the first frame, and to the first pixels, meaning the
// window server's copy of the window showing content.
//
//   appkit_floor             default window (with AppKit's open animation)
//   appkit_floor --no-anim   animation off, as zcode opens its windows

import AppKit
import Darwin

func processStartMs() -> Double {
    var info = proc_bsdinfo()
    _ = proc_pidinfo(getpid(), PROC_PIDTBSDINFO, 0, &info, Int32(MemoryLayout<proc_bsdinfo>.size))
    return Double(info.pbi_start_tvsec) * 1000 + Double(info.pbi_start_tvusec) / 1000
}

func nowMs() -> Double {
    var tv = timeval()
    gettimeofday(&tv, nil)
    return Double(tv.tv_sec) * 1000 + Double(tv.tv_usec) / 1000
}

// CGWindowListCreateImage is unavailable to Swift in the current SDK but still
// works for a process's own windows; look it up at runtime.
typealias CreateImage = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?
let createImage = unsafeBitCast(
    dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGWindowListCreateImage"), to: CreateImage.self)

/// Same test as zcode's `debug::window_shows_content`: below the top 15%,
/// more than 0.1% of sampled pixels differ from the first one.
func windowShowsContent(_ number: UInt32) -> Bool? {
    guard let image = createImage(.null, 1 << 3, number, 1 | (1 << 9))?.takeRetainedValue(),
          image.bitsPerPixel == 32,
          let data = image.dataProvider?.data, let bytes = CFDataGetBytePtr(data)
    else { return nil }
    let (width, height, rowBytes) = (image.width, image.height, image.bytesPerRow)
    let length = CFDataGetLength(data)
    let first = (height * 15 / 100) * rowBytes
    guard first + 4 <= length else { return nil }
    var total = 0, differing = 0
    for y in stride(from: height * 15 / 100, to: height * 95 / 100, by: 2) {
        for x in stride(from: width * 5 / 100, to: width * 95 / 100, by: 2) {
            let i = y * rowBytes + x * 4
            guard i + 4 <= length else { continue }
            total += 1
            if (0..<4).contains(where: { abs(Int(bytes[i + $0]) - Int(bytes[first + $0])) > 24 }) {
                differing += 1
            }
        }
    }
    return total > 0 && differing * 1000 > total
}

var firstFrameMs: Double?
var firstPixelsMs: Double?

func finishIfDone() {
    guard let frame = firstFrameMs, let pixels = firstPixelsMs else { return }
    print(String(format: "{\"total_to_first_frame_ms\":%.1f,\"total_to_first_pixels_ms\":%.1f}", frame, pixels))
    exit(0)
}

final class View: NSView {
    var drawn = false

    override func draw(_ dirtyRect: NSRect) {
        NSColor.windowBackgroundColor.setFill()
        dirtyRect.fill()
        ("Welcome" as NSString).draw(
            at: NSPoint(x: 40, y: 200),
            withAttributes: [.font: NSFont.systemFont(ofSize: 24)])
        guard !drawn else { return }
        drawn = true
        // Like zcode's measurement, stop on the main loop's next turn after
        // the first frame was drawn.
        DispatchQueue.main.async {
            firstFrameMs = nowMs() - processStartMs()
            finishIfDone()
        }
    }
}

final class Delegate: NSObject, NSApplicationDelegate {
    var window: NSWindow!

    func applicationDidFinishLaunching(_ notification: Notification) {
        window = NSWindow(
            contentRect: NSRect(x: 200, y: 200, width: 820, height: 480),
            styleMask: [.titled, .closable, .resizable, .miniaturizable],
            backing: .buffered, defer: false)
        window.contentView = View()
        if CommandLine.arguments.contains("--no-anim") {
            window.animationBehavior = .none
        }
        window.makeKeyAndOrderFront(nil)
        let number = UInt32(window.windowNumber)
        Thread.detachNewThread {
            while windowShowsContent(number) != true { usleep(1000) }
            let t = nowMs() - processStartMs()
            DispatchQueue.main.async {
                firstPixelsMs = t
                finishIfDone()
            }
        }
        NSApp.activate(ignoringOtherApps: true)
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let delegate = Delegate()
app.delegate = delegate
app.run()

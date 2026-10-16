// A minimal native AppKit app: one window with a line of text. scripts/e2e.py
// builds it with `swiftc -O` and compares zcode's cold launch against it.
// Prints the time from process start to the first frame as JSON, measured
// like `zcode --startup-bench`, then quits.
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
            print(String(format: "{\"total_to_first_frame_ms\":%.1f}", nowMs() - processStartMs()))
            exit(0)
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
        NSApp.activate(ignoringOtherApps: true)
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let delegate = Delegate()
app.delegate = delegate
app.run()

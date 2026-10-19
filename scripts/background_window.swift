// A small window in the screen's bottom-left corner, for scripts/e2e.py's
// idle checks: brought to the front (through System Events: an app started
// from a script can't activate itself), it makes zcode's window inactive
// while leaving it visible. Quits after the seconds given (default 10).
//
//   background_window [seconds]

import AppKit

let seconds = Double(CommandLine.arguments.dropFirst().first ?? "10") ?? 10
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 160, height: 60), styleMask: [.titled], backing: .buffered, defer: false)
window.title = "zcode e2e"
window.setFrameOrigin(NSPoint(x: 0, y: 0))
window.makeKeyAndOrderFront(nil)
DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { app.terminate(nil) }
app.run()

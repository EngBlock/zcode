// Times how long an app takes from launch to its first window on screen,
// measured from outside the app so every app is timed the same way.
//
//   launch_timer [--runs N] [--keep-running] -- /path/to/executable [args...]
//
// Spawns the executable directly (not through `open`), polls the window
// server's on-screen window list every millisecond until the process owns a
// normal-level window at least 300x200 points, prints the time as JSON, and
// terminates the process. The window list needs no Screen Recording
// permission.

import AppKit
import Foundation

func nowMs() -> Double { Double(DispatchTime.now().uptimeNanoseconds) / 1e6 }

func hasWindow(pid: pid_t) -> Bool {
    guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
        as? [[String: Any]]
    else { return false }
    return list.contains { info in
        guard (info[kCGWindowOwnerPID as String] as? pid_t) == pid,
              (info[kCGWindowLayer as String] as? Int) == 0,
              (info[kCGWindowAlpha as String] as? Double ?? 1) > 0,
              let bounds = info[kCGWindowBounds as String] as? [String: Double]
        else { return false }
        return (bounds["Width"] ?? 0) >= 300 && (bounds["Height"] ?? 0) >= 200
    }
}

var args = Array(CommandLine.arguments.dropFirst())
var runs = 1
if let i = args.firstIndex(of: "--runs") { runs = Int(args[i + 1])!; args.removeSubrange(i...i + 1) }
guard let dash = args.firstIndex(of: "--"), dash + 1 < args.count else {
    FileHandle.standardError.write("usage: launch_timer [--runs N] -- executable [args...]\n".data(using: .utf8)!)
    exit(2)
}
let executable = args[dash + 1]
let appArgs = Array(args[(dash + 2)...])

var results: [Double] = []
for _ in 0..<runs {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = appArgs
    process.standardOutput = FileHandle.nullDevice
    process.standardError = FileHandle.nullDevice
    let start = nowMs()
    try process.run()
    let pid = process.processIdentifier
    var shown: Double?
    while nowMs() - start < 30_000 && process.isRunning {
        if hasWindow(pid: pid) { shown = nowMs() - start; break }
        usleep(1000)
    }
    // Let the app settle before quitting so the next launch starts clean.
    usleep(500_000)
    process.terminate()
    let deadline = nowMs() + 3000
    while process.isRunning && nowMs() < deadline { usleep(10_000) }
    if process.isRunning { kill(pid, SIGKILL); process.waitUntilExit() }
    usleep(500_000)
    guard let shown else {
        print("{\"error\":\"no window within 30 s (exit status \(process.terminationStatus))\"}")
        exit(1)
    }
    results.append(shown)
}
let sorted = results.sorted()
let runsJSON = results.map { String(format: "%.0f", $0) }.joined(separator: ",")
print(String(format: "{\"median_ms\":%.0f,\"runs_ms\":[%@]}", sorted[sorted.count / 2], runsJSON))

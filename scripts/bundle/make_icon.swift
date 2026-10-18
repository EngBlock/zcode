// Draws zcode's app icon and writes it as an .icns:
//
//   swift scripts/bundle/make_icon.swift <out.icns> [--png <preview.png>]
//
// macOS's icon grid: a 1024-point canvas with the rounded square on 824
// points, as Apple's template has it. The square is a dark slate gradient;
// on it a "z" of three strokes in a blue-to-cyan gradient, the top and
// bottom bars joined by the diagonal, and a thin highlight along the top
// edge. Every size of the iconset is drawn, not scaled, so small ones stay
// crisp; iconutil packs them.

import AppKit
import Foundation

func draw(size: CGFloat) -> NSBitmapImageRep {
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size), bitsPerSample: 8,
        samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
        bytesPerRow: 0, bitsPerPixel: 0)!
    let context = NSGraphicsContext(bitmapImageRep: rep)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = context
    let cg = context.cgContext
    cg.clear(CGRect(x: 0, y: 0, width: size, height: size))
    let unit = size / 1024
    cg.scaleBy(x: unit, y: unit)

    // The square, with a soft shadow below it as Apple's icons have.
    let square = CGRect(x: 100, y: 100, width: 824, height: 824)
    let radius: CGFloat = 185
    let shape = CGPath(roundedRect: square, cornerWidth: radius, cornerHeight: radius, transform: nil)
    cg.saveGState()
    cg.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: CGColor(gray: 0, alpha: 0.35))
    cg.addPath(shape)
    cg.setFillColor(CGColor(red: 0.11, green: 0.13, blue: 0.18, alpha: 1))
    cg.fillPath()
    cg.restoreGState()

    cg.saveGState()
    cg.addPath(shape)
    cg.clip()
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let slate = CGGradient(
        colorsSpace: space,
        colors: [
            CGColor(red: 0.20, green: 0.24, blue: 0.32, alpha: 1),
            CGColor(red: 0.07, green: 0.09, blue: 0.13, alpha: 1),
        ] as CFArray, locations: [0, 1])!
    cg.drawLinearGradient(slate, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 100), options: [])

    // The "z": a top bar, the diagonal and a bottom bar, one path so the
    // gradient runs across all three.
    let z = CGMutablePath()
    let t: CGFloat = 96 // stroke thickness
    let left: CGFloat = 292, right: CGFloat = 732, top: CGFloat = 724, bottom: CGFloat = 300
    z.move(to: CGPoint(x: left, y: top))
    z.addLine(to: CGPoint(x: right, y: top))
    z.addLine(to: CGPoint(x: right, y: top - t))
    z.addLine(to: CGPoint(x: left + 150, y: bottom + t))
    z.addLine(to: CGPoint(x: right, y: bottom + t))
    z.addLine(to: CGPoint(x: right, y: bottom))
    z.addLine(to: CGPoint(x: left, y: bottom))
    z.addLine(to: CGPoint(x: left, y: bottom + t))
    z.addLine(to: CGPoint(x: right - 150, y: top - t))
    z.addLine(to: CGPoint(x: left, y: top - t))
    z.closeSubpath()
    cg.saveGState()
    cg.setShadow(offset: CGSize(width: 0, height: -6), blur: 18, color: CGColor(gray: 0, alpha: 0.45))
    cg.addPath(z)
    cg.setFillColor(CGColor(red: 0.35, green: 0.62, blue: 1, alpha: 1))
    cg.fillPath()
    cg.restoreGState()
    cg.saveGState()
    cg.addPath(z)
    cg.clip()
    let blue = CGGradient(
        colorsSpace: space,
        colors: [
            CGColor(red: 0.55, green: 0.86, blue: 1, alpha: 1),
            CGColor(red: 0.27, green: 0.52, blue: 1, alpha: 1),
        ] as CFArray, locations: [0, 1])!
    cg.drawLinearGradient(blue, start: CGPoint(x: 300, y: 740), end: CGPoint(x: 740, y: 290), options: [])
    cg.restoreGState()

    // A highlight along the top edge of the square.
    let highlight = CGGradient(
        colorsSpace: space,
        colors: [CGColor(gray: 1, alpha: 0.22), CGColor(gray: 1, alpha: 0)] as CFArray, locations: [0, 1])!
    cg.drawLinearGradient(highlight, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 860), options: [])
    cg.restoreGState()

    // The rim.
    cg.addPath(shape)
    cg.setStrokeColor(CGColor(gray: 1, alpha: 0.10))
    cg.setLineWidth(4)
    cg.strokePath()

    NSGraphicsContext.restoreGraphicsState()
    return rep
}

let args = Array(CommandLine.arguments.dropFirst())
guard let out = args.first else {
    FileHandle.standardError.write("usage: make_icon.swift <out.icns> [--png <preview.png>]\n".data(using: .utf8)!)
    exit(2)
}
let iconset = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("zcode-\(getpid()).iconset")
try FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)
for (points, scale) in [(16, 1), (16, 2), (32, 1), (32, 2), (128, 1), (128, 2), (256, 1), (256, 2), (512, 1), (512, 2)] {
    let rep = draw(size: CGFloat(points * scale))
    let name = scale == 1 ? "icon_\(points)x\(points).png" : "icon_\(points)x\(points)@2x.png"
    try rep.representation(using: .png, properties: [:])!.write(to: iconset.appendingPathComponent(name))
}
if let i = args.firstIndex(of: "--png"), i + 1 < args.count {
    try draw(size: 512).representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[i + 1]))
}
let iconutil = Process()
iconutil.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
iconutil.arguments = ["-c", "icns", iconset.path, "-o", out]
try iconutil.run()
iconutil.waitUntilExit()
try? FileManager.default.removeItem(at: iconset)
exit(iconutil.terminationStatus)

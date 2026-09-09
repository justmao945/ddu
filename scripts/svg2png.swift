// Usage: svg2png.swift <src.svg> <dst.png> <size>
// Rasterizes SVG via AppKit's native SVG support (macOS 11+), preserving
// alpha, at exact pixel dimensions (independent of screen scale).
//
// Used by make-icon.sh instead of `qlmanage`: QuickLook rasterizes onto an
// opaque white canvas, which bakes a white square behind the icon's rounded
// rect — visible as a white halo in the Dock. Requires the Xcode CLT
// (already needed to link Rust).

import AppKit

let a = CommandLine.arguments
guard a.count >= 4, let size = Int(a[3]), let img = NSImage(contentsOfFile: a[1]) else {
    FileHandle.standardError.write(Data("usage: svg2png <src.svg> <dst.png> <size>\n".utf8))
    exit(1)
}
let px = CGFloat(size)
guard let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size,
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .calibratedRGB, bytesPerRow: 0, bitsPerPixel: 0)
else {
    FileHandle.standardError.write(Data("bitmap rep failed\n".utf8))
    exit(1)
}
rep.size = NSSize(width: px, height: px)
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
NSGraphicsContext.current?.imageInterpolation = .high
img.draw(in: NSRect(x: 0, y: 0, width: px, height: px),
         from: .zero, operation: .sourceOver, fraction: 1.0)
NSGraphicsContext.restoreGraphicsState()
guard let png = rep.representation(using: .png, properties: [:]) else {
    FileHandle.standardError.write(Data("png encode failed\n".utf8))
    exit(1)
}
try! png.write(to: URL(fileURLWithPath: a[2]))

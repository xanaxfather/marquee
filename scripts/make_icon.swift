// Renders the Marquee app icon with Core Graphics.
//
//   swift scripts/make_icon.swift mac   assets/icon-mac-1024.png   # macOS grid, margin + shadow
//   swift scripts/make_icon.swift win   assets/icon-win-1024.png   # full-bleed rounded square
//   swift scripts/make_icon.swift small assets/icon-small-256.png  # 16–24 px: play glyph only
//
// One flat brand colour, one white glyph (a screen with a play button).
import AppKit
import CoreGraphics

let variant = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "mac"
let out = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "icon.png"
let S: CGFloat = variant == "small" ? 256 : 1024
let cs = CGColorSpace(name: CGColorSpace.sRGB)!
let ctx = CGContext(data: nil, width: Int(S), height: Int(S), bitsPerComponent: 8, bytesPerRow: 0,
                    space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!

func rgb(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(colorSpace: cs, components: [CGFloat((hex >> 16) & 0xff) / 255, CGFloat((hex >> 8) & 0xff) / 255,
                                         CGFloat(hex & 0xff) / 255, a])!
}
func rounded(_ r: CGRect, _ radius: CGFloat) -> CGPath {
    CGPath(roundedRect: r, cornerWidth: radius, cornerHeight: radius, transform: nil)
}
func roundedTriangle(_ a: CGPoint, _ b: CGPoint, _ c: CGPoint, r: CGFloat) -> CGPath {
    let p = CGMutablePath()
    p.move(to: CGPoint(x: (a.x + b.x) / 2, y: (a.y + b.y) / 2))
    p.addArc(tangent1End: b, tangent2End: c, radius: r)
    p.addArc(tangent1End: c, tangent2End: a, radius: r)
    p.addArc(tangent1End: a, tangent2End: b, radius: r)
    p.closeSubpath()
    return p
}
/// Play triangle centred (optically) on `c`, `h` tall.
func play(_ c: CGPoint, _ h: CGFloat, r: CGFloat) -> CGPath {
    let w = h * 0.88
    let x0 = c.x - w * 0.42   // optical centre sits left of the bounding-box centre
    return roundedTriangle(CGPoint(x: x0, y: c.y + h / 2), CGPoint(x: x0, y: c.y - h / 2), CGPoint(x: x0 + w, y: c.y), r: r)
}

// ---- Tile
let tile: CGRect
let radius: CGFloat
switch variant {
case "mac": tile = CGRect(x: 100, y: 100, width: 824, height: 824); radius = 186
case "win": tile = CGRect(x: 24, y: 24, width: 976, height: 976); radius = 200
default: tile = CGRect(x: 6, y: 6, width: 244, height: 244); radius = 56
}
let tilePath = rounded(tile, radius)
if variant == "mac" {
    // A single soft shadow, as macOS icons use.
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 28, color: rgb(0x000000, 0.28))
    ctx.addPath(tilePath); ctx.setFillColor(rgb(0x5865f2)); ctx.fillPath()
    ctx.restoreGState()
}
// Flat brand colour with a barely-there vertical gradient for depth.
ctx.saveGState(); ctx.addPath(tilePath); ctx.clip()
let g = CGGradient(colorsSpace: cs, colors: [rgb(0x6672f6), rgb(0x4f5ae4)] as CFArray, locations: [0, 1])!
ctx.drawLinearGradient(g, start: CGPoint(x: 0, y: tile.maxY), end: CGPoint(x: 0, y: tile.minY), options: [])
ctx.restoreGState()

// ---- Glyph
ctx.setFillColor(rgb(0xffffff))
ctx.setStrokeColor(rgb(0xffffff))
let c = CGPoint(x: tile.midX, y: tile.midY)
if variant == "small" {
    ctx.addPath(play(c, tile.height * 0.50, r: 14)); ctx.fillPath()
} else {
    let k = tile.width / 824          // glyph scales with the tile
    let screen = CGRect(x: c.x - 270 * k, y: c.y - 196 * k, width: 540 * k, height: 392 * k)
    ctx.setLineWidth(52 * k)
    ctx.addPath(rounded(screen.insetBy(dx: 26 * k, dy: 26 * k), 64 * k)); ctx.strokePath()
    ctx.addPath(play(c, 180 * k, r: 22 * k)); ctx.fillPath()
}

let rep = NSBitmapImageRep(cgImage: ctx.makeImage()!)
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
print("wrote \(out)")

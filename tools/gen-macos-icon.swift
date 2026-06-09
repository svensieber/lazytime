import AppKit

func parseSizeArg(_ raw: String) -> CGFloat? {
    let value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    guard let intValue = Int(value), intValue > 0 else {
        return nil
    }
    return CGFloat(intValue)
}

func drawIcon(size: CGFloat) -> NSImage {
    let image = NSImage(size: NSSize(width: size, height: size))
    image.lockFocus()

    let rect = NSRect(x: 0, y: 0, width: size, height: size)
    let cornerRadius = size * 0.225
    let iconPath = NSBezierPath(roundedRect: rect, xRadius: cornerRadius, yRadius: cornerRadius)
    iconPath.addClip()

    let bg = NSGradient(colors: [
        NSColor(calibratedRed: 0.09, green: 0.25, blue: 0.56, alpha: 1.0),
        NSColor(calibratedRed: 0.13, green: 0.60, blue: 0.82, alpha: 1.0),
    ])
    bg?.draw(in: rect, angle: -90)

    let highlightRect = NSRect(x: 0, y: size * 0.45, width: size, height: size * 0.55)
    let highlight = NSGradient(colors: [
        NSColor(calibratedWhite: 1.0, alpha: 0.34),
        NSColor(calibratedWhite: 1.0, alpha: 0.02),
    ])
    highlight?.draw(in: highlightRect, angle: 90)

    let bezelInset = size * 0.11
    let bezelRect = rect.insetBy(dx: bezelInset, dy: bezelInset)
    let bezelPath = NSBezierPath(ovalIn: bezelRect)
    NSColor(calibratedWhite: 1.0, alpha: 0.18).setFill()
    bezelPath.fill()

    let faceInset = size * 0.18
    let faceRect = rect.insetBy(dx: faceInset, dy: faceInset)
    let facePath = NSBezierPath(ovalIn: faceRect)
    NSColor(calibratedWhite: 1.0, alpha: 0.95).setFill()
    facePath.fill()

    let tickColor = NSColor(calibratedRed: 0.10, green: 0.42, blue: 0.66, alpha: 0.75)
    tickColor.setStroke()
    let tickPath = NSBezierPath()
    tickPath.lineWidth = max(1.2, size * 0.02)

    let center = CGPoint(x: size / 2.0, y: size / 2.0)
    let radius = faceRect.width * 0.42
    let innerRadius = radius * 0.78

    for index in 0..<12 {
        let angle = (CGFloat(index) / 12.0) * .pi * 2.0 - .pi / 2.0
        let x1 = center.x + cos(angle) * innerRadius
        let y1 = center.y + sin(angle) * innerRadius
        let x2 = center.x + cos(angle) * radius
        let y2 = center.y + sin(angle) * radius
        tickPath.move(to: CGPoint(x: x1, y: y1))
        tickPath.line(to: CGPoint(x: x2, y: y2))
    }
    tickPath.stroke()

    let handColor = NSColor(calibratedRed: 0.06, green: 0.24, blue: 0.44, alpha: 1.0)
    handColor.setStroke()

    let minuteHand = NSBezierPath()
    minuteHand.lineWidth = max(1.6, size * 0.045)
    minuteHand.lineCapStyle = .round
    minuteHand.move(to: center)
    minuteHand.line(to: CGPoint(x: center.x, y: center.y + radius * 0.62))
    minuteHand.stroke()

    let hourHand = NSBezierPath()
    hourHand.lineWidth = max(1.6, size * 0.055)
    hourHand.lineCapStyle = .round
    hourHand.move(to: center)
    hourHand.line(to: CGPoint(x: center.x + radius * 0.42, y: center.y + radius * 0.16))
    hourHand.stroke()

    let dotRect = NSRect(
        x: center.x - size * 0.05,
        y: center.y - size * 0.05,
        width: size * 0.10,
        height: size * 0.10
    )
    let dotPath = NSBezierPath(ovalIn: dotRect)
    handColor.setFill()
    dotPath.fill()

    iconPath.lineWidth = max(1.0, size * 0.012)
    NSColor(calibratedWhite: 1.0, alpha: 0.30).setStroke()
    iconPath.stroke()

    image.unlockFocus()
    return image
}

func savePng(image: NSImage, destination: String) throws {
    guard
        let tiffData = image.tiffRepresentation,
        let bitmap = NSBitmapImageRep(data: tiffData),
        let pngData = bitmap.representation(using: .png, properties: [:])
    else {
        throw NSError(domain: "LazyTimeIcon", code: 1)
    }

    try pngData.write(to: URL(fileURLWithPath: destination))
}

if CommandLine.arguments.count != 3 {
    fputs("Usage: gen-macos-icon.swift <size> <output-png>\n", stderr)
    exit(1)
}

guard let size = parseSizeArg(CommandLine.arguments[1]) else {
    fputs("Invalid icon size\n", stderr)
    exit(1)
}

let outPath = CommandLine.arguments[2]
let image = drawIcon(size: size)

do {
    try savePng(image: image, destination: outPath)
} catch {
    fputs("Failed to write PNG: \(error)\n", stderr)
    exit(1)
}

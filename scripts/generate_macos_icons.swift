#!/usr/bin/env swift

import AppKit
import Foundation

/// Generates a macOS iconset PNG family for WhisperInput.
struct IconGenerator {
    /// Writes the full iconset into the provided directory.
    static func writeIconset(to outputDirectory: URL) throws {
        let variants: [(String, CGFloat)] = [
            ("icon_16x16.png", 16),
            ("icon_16x16@2x.png", 32),
            ("icon_32x32.png", 32),
            ("icon_32x32@2x.png", 64),
            ("icon_128x128.png", 128),
            ("icon_128x128@2x.png", 256),
            ("icon_256x256.png", 256),
            ("icon_256x256@2x.png", 512),
            ("icon_512x512.png", 512),
            ("icon_512x512@2x.png", 1024),
        ]

        try FileManager.default.createDirectory(
            at: outputDirectory,
            withIntermediateDirectories: true
        )

        for (name, size) in variants {
            let imageData = try makePNG(size: size)
            try imageData.write(to: outputDirectory.appendingPathComponent(name))
        }
    }

    /// Renders one rounded-square app icon PNG.
    static func makePNG(size: CGFloat) throws -> Data {
        let pixelSize = NSSize(width: size, height: size)
        let rect = NSRect(origin: .zero, size: pixelSize)
        guard let rep = NSBitmapImageRep(
            bitmapDataPlanes: nil,
            pixelsWide: Int(size),
            pixelsHigh: Int(size),
            bitsPerSample: 8,
            samplesPerPixel: 4,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: .deviceRGB,
            bytesPerRow: 0,
            bitsPerPixel: 0
        ),
        let context = NSGraphicsContext(bitmapImageRep: rep)
        else {
            throw NSError(domain: "WhisperInputIcon", code: 1)
        }

        rep.size = pixelSize
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = context
        defer { NSGraphicsContext.restoreGraphicsState() }

        drawBackground(in: rect)
        drawWaveform(in: rect)
        drawHighlight(in: rect)

        guard let png = rep.representation(using: .png, properties: [:]) else {
            throw NSError(domain: "WhisperInputIcon", code: 1)
        }

        return png
    }

    /// Draws the warm gradient background used for the application icon.
    static func drawBackground(in rect: NSRect) {
        let background = NSBezierPath(
            roundedRect: rect.insetBy(dx: rect.width * 0.04, dy: rect.height * 0.04),
            xRadius: rect.width * 0.24,
            yRadius: rect.height * 0.24
        )

        let gradient = NSGradient(colors: [
            NSColor(calibratedRed: 1.0, green: 0.54, blue: 0.31, alpha: 1.0),
            NSColor(calibratedRed: 0.93, green: 0.28, blue: 0.25, alpha: 1.0),
        ])!
        gradient.draw(in: background, angle: 90)
    }

    /// Draws the center waveform mark.
    static func drawWaveform(in rect: NSRect) {
        let heights: [CGFloat] = [0.24, 0.46, 0.72, 0.46, 0.24]
        let barWidth = rect.width * 0.085
        let gap = rect.width * 0.04
        let totalWidth = CGFloat(heights.count) * barWidth + CGFloat(heights.count - 1) * gap
        let startX = rect.midX - totalWidth / 2

        for (index, fraction) in heights.enumerated() {
            let height = rect.height * fraction
            let x = startX + CGFloat(index) * (barWidth + gap)
            let barRect = NSRect(
                x: x,
                y: rect.midY - height / 2,
                width: barWidth,
                height: height
            )
            let barPath = NSBezierPath(
                roundedRect: barRect,
                xRadius: barWidth / 2,
                yRadius: barWidth / 2
            )
            NSColor(calibratedWhite: 1.0, alpha: 0.96).setFill()
            barPath.fill()
        }
    }

    /// Adds a subtle highlight to keep the icon from feeling flat.
    static func drawHighlight(in rect: NSRect) {
        let highlightRect = NSRect(
            x: rect.width * 0.16,
            y: rect.height * 0.58,
            width: rect.width * 0.68,
            height: rect.height * 0.22
        )
        let path = NSBezierPath(roundedRect: highlightRect, xRadius: rect.width * 0.11, yRadius: rect.width * 0.11)
        NSColor(calibratedWhite: 1.0, alpha: 0.10).setFill()
        path.fill()
    }
}

let arguments = CommandLine.arguments
guard arguments.count == 2 else {
    fputs("usage: generate_macos_icons.swift <iconset-output-dir>\n", stderr)
    exit(64)
}

let outputDirectory = URL(fileURLWithPath: arguments[1], isDirectory: true)

do {
    try IconGenerator.writeIconset(to: outputDirectory)
} catch {
    fputs("failed to generate iconset: \(error)\n", stderr)
    exit(1)
}

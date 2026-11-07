import SwiftUI
import AppKit

struct ReadingOverlayView: View {
    var presentation: ReadingPresentation
    var scale: Double
    var selected: Bool
    var select: () -> Void

    private func polygon(_ points: [[Double]], origin: CGPoint) -> Path {
        Path { path in
            let valid = points.filter { $0.count == 2 && $0.allSatisfy(\.isFinite) }
            guard let first = valid.first else { return }
            path.move(to: CGPoint(x: (first[0] - origin.x) * scale, y: (first[1] - origin.y) * scale))
            for point in valid.dropFirst() { path.addLine(to: CGPoint(x: (point[0] - origin.x) * scale, y: (point[1] - origin.y) * scale)) }
            path.closeSubpath()
        }
    }

    var body: some View {
        let region = presentation.region
        let rect = region.rectangle ?? .zero
        let coordinates = region.points.filter { $0.count == 2 }
        let left = min(rect.minX, coordinates.map { $0[0] }.min() ?? rect.minX)
        let top = min(rect.minY, coordinates.map { $0[1] }.min() ?? rect.minY)
        let right = max(rect.maxX, coordinates.map { $0[0] }.max() ?? rect.maxX)
        let bottom = max(rect.maxY, coordinates.map { $0[1] }.max() ?? rect.maxY)
        let origin = CGPoint(x: left, y: top)
        let components = region.fill_color.count == 3 ? region.fill_color : [255, 255, 255]
        let fill = Color(red: components[0] / 255, green: components[1] / 255, blue: components[2] / 255)
        Canvas { context, _ in
            let contour = polygon(region.points, origin: origin)
            context.clip(to: contour)
            if let encoded = region.background_png, let box = region.background_box,
               box.count == 4, let data = Data(base64Encoded: encoded), let restored = NSImage(data: data) {
                let patch = Image(nsImage: restored)
                context.draw(patch, in: CGRect(x: (box[0] - left) * scale, y: (box[1] - top) * scale,
                                              width: (box[2] - box[0]) * scale, height: (box[3] - box[1]) * scale))
            } else if region.background_png == nil {
                for erase in region.erase_points ?? [region.points] {
                    context.fill(polygon(erase, origin: origin), with: .color(fill))
                }
            }
            for glyph in presentation.layout.glyphs {
                if region.background_png != nil {
                    // A small glyph outline keeps text readable over screentone without a white card.
                    let stroke = max(0.5, presentation.layout.fontSize * 0.055) * scale
                    for direction in 0..<8 {
                        let angle = Double(direction) * .pi / 4
                        context.draw(Text(glyph.text).font(.system(size: presentation.layout.fontSize * scale, weight: .medium)).foregroundColor(.white),
                                     at: CGPoint(x: (glyph.center.x - left) * scale + cos(angle) * stroke,
                                                 y: (glyph.center.y - top) * scale + sin(angle) * stroke))
                    }
                }
                context.draw(Text(glyph.text).font(.system(size: presentation.layout.fontSize * scale, weight: .medium)).foregroundColor(.black),
                             at: CGPoint(x: (glyph.center.x - left) * scale, y: (glyph.center.y - top) * scale))
            }
            if selected { context.stroke(contour, with: .color(.teal), lineWidth: 2) }
        }
        .frame(width: max(1, (right - left) * scale), height: max(1, (bottom - top) * scale))
        .contentShape(polygon(region.points, origin: origin))
        .onTapGesture(perform: select)
        .accessibilityLabel(presentation.text)
        .help(presentation.text)
        .offset(x: left * scale, y: top * scale)
    }
}

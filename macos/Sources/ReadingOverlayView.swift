import SwiftUI
import AppKit

// Draw every repair before drawing any translated glyphs. A moved dialogue can
// cross a different repaired balloon in both the reader and the exported PNG.
struct ReadingPageOverlays: View {
    let items: [ReadingPresentation]
    let scale: Double
    let verticalScale: Double
    var selectRegion: ((ReadingPresentation) -> Void)? = nil
    var body: some View {
        ZStack(alignment: .topLeading) {
            ForEach(items) { item in
                ReadingOverlayView(presentation: item, scale: scale, verticalScale: verticalScale,
                                   selected: false, layer: .background, select: { selectRegion?(item) })
                    .allowsHitTesting(selectRegion != nil)
            }
            ForEach(items) { item in
                ReadingOverlayView(presentation: item, scale: scale, verticalScale: verticalScale,
                                   selected: false, layer: .text, select: {})
                    .allowsHitTesting(false)
            }
        }
    }
}

struct ReadingOverlayView: View {
    enum Layer { case background, text, complete }
    var presentation: ReadingPresentation
    var scale: Double
    var verticalScale: Double? = nil
    var selected: Bool
    var selectionColor: Color = .accentColor
    var layer: Layer = .complete
    var select: () -> Void

    private var yScale: Double { verticalScale ?? scale }

    private func polygon(_ points: [[Double]], origin: CGPoint) -> Path {
        Path { path in
            let valid = points.filter { $0.count == 2 && $0.allSatisfy(\.isFinite) }
            guard let first = valid.first else { return }
            path.move(to: CGPoint(x: (first[0] - origin.x) * scale, y: (first[1] - origin.y) * yScale))
            for point in valid.dropFirst() { path.addLine(to: CGPoint(x: (point[0] - origin.x) * scale, y: (point[1] - origin.y) * yScale)) }
            path.closeSubpath()
        }
    }

    var body: some View {
        let region = presentation.region
        let rect = region.rectangle ?? .zero
        let coordinates = region.points.filter { $0.count == 2 }
        let textBounds = presentation.runs.reduce(CGRect.null) { bounds, run in
            bounds.union(run.layout.bounds.insetBy(dx: -run.layout.fontSize * 0.35, dy: -run.layout.fontSize * 0.35))
        }
        let regionBounds = coordinates.reduce(rect) { bounds, point in
            bounds.union(CGRect(x: point[0], y: point[1], width: 0, height: 0))
        }
        let bounds: CGRect = switch layer {
        case .text: textBounds
        case .background: regionBounds
        case .complete: regionBounds.union(textBounds)
        }
        let origin = bounds.origin, left = bounds.minX, top = bounds.minY
        let components = region.fill_color.count == 3 ? region.fill_color : [255, 255, 255]
        let fill = Color(red: components[0] / 255, green: components[1] / 255, blue: components[2] / 255)
        let textColor: Color = region.dark_background == true ? .white : .black
        let outlineColor: Color = region.dark_background == true ? .black : .white
        Canvas { context, _ in
            let contour = polygon(region.points, origin: origin)
            var background = context
            background.clip(to: contour)
            if layer != .text, let restored = presentation.background, let box = region.background_box {
                let patch = Image(decorative: restored, scale: 1)
                background.draw(patch, in: CGRect(x: (box[0] - left) * scale, y: (box[1] - top) * yScale,
                                              width: (box[2] - box[0]) * scale, height: (box[3] - box[1]) * yScale))
            } else if layer != .text, region.background_png == nil {
                for erase in region.erase_points ?? [region.points] {
                    background.fill(polygon(erase, origin: origin), with: .color(fill))
                }
            }
            guard layer != .background else { return }
            for run in presentation.runs {
                var ink = context
                if let frame = run.manualFrame {
                    ink.clip(to: Path(CGRect(x: (frame.minX - left) * scale, y: (frame.minY - top) * yScale,
                                             width: frame.width * scale, height: frame.height * yScale)))
                } else { ink.clip(to: contour) }
                if scale > 0 { ink.scaleBy(x: 1, y: yScale / scale) }
                let layout = run.layout
                let size = layout.fontSize * scale
                let font = layout.fontName.map { Font.custom($0, fixedSize: size) } ?? .system(size: size, weight: .medium)
                let stroke = max(0.5, layout.fontSize * 0.055) * scale
                for glyph in layout.glyphs {
                    let center = CGPoint(x: (glyph.center.x - left) * scale, y: (glyph.center.y - top) * scale)
                    let character = Text(glyph.text).font(font)
                    if presentation.background != nil {
                        let outline = ink.resolve(character.foregroundColor(outlineColor))
                        for direction in 0..<8 {
                            let angle = Double(direction) * .pi / 4
                            ink.draw(outline, at: CGPoint(x: center.x + cos(angle) * stroke,
                                                         y: center.y + sin(angle) * stroke))
                        }
                    }
                    ink.draw(character.foregroundColor(textColor), at: center)
                }
            }
            if selected { context.stroke(contour, with: .color(selectionColor), lineWidth: 2) }
        }
        .frame(width: max(1, bounds.width * scale), height: max(1, bounds.height * yScale))
        .contentShape(polygon(region.points, origin: origin))
        .onTapGesture(perform: select)
        .accessibilityLabel(presentation.text)
        .help(presentation.text)
        .offset(x: left * scale, y: top * yScale)
    }
}

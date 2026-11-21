import Foundation
import CoreGraphics
import CoreText
import ImageIO
import NaturalLanguage
import AppKit

struct OCRReview: Codable {
    var status: String
    var reason: String
}

struct TextBlock: Codable, Identifiable {
    var id: String
    var box: [Double]
    var source: String
    var translation: String
    var error: String
    var retranslation_error: String?
    var vertical: Bool
    var edited: Bool
    var vision_source: String?
    var quality_warnings: [String]?
    var keep_original: Bool?
    var overlay: String?
    var text_kind: String?
    var ocr_review: OCRReview?
    var ocr_confirmed: Bool?

    var translationIssue: String? {
        if !error.isEmpty { return error }
        if let reason = retranslation_error, !reason.isEmpty { return "重译未完成，已保留旧译文。\(reason)" }
        return nil
    }

    var keepsOriginal: Bool { keep_original == true }
    var needsOCRReview: Bool { !keepsOriginal && ocr_review?.status == "suspect" && ocr_confirmed != true && !edited }

    var needsReadingOverlay: Bool {
        guard !keepsOriginal, !needsOCRReview else { return false }
        let original = source.precomposedStringWithCompatibilityMapping.trimmingCharacters(in: .whitespacesAndNewlines)
        let translated = translation.precomposedStringWithCompatibilityMapping.trimmingCharacters(in: .whitespacesAndNewlines)
        return !translated.isEmpty && error.isEmpty && original != translated
    }
}

struct PageResult: Codable {
    var version: Int
    var width: Int
    var height: Int
    var blocks: [TextBlock]
    var model: String
    var ocr: String
    var overlay_regions: [ReadingRegion]?
}

struct ReadingRegion: Codable, Identifiable {
    var id: String
    var block_ids: [String]
    var points: [[Double]]
    var text_box: [Double]
    var erase_points: [[[Double]]]?
    var fill_color: [Double]
    var vertical: Bool
    var overlay: String?
    var background_png: String?
    var background_box: [Double]?
    var dark_background: Bool?

    var decodedBackground: CGImage? {
        guard let encoded = background_png, let box = background_box,
              box.count == 4, box.allSatisfy(\.isFinite),
              let data = Data(base64Encoded: encoded),
              data.starts(with: [137, 80, 78, 71, 13, 10, 26, 10]),
              let source = CGImageSourceCreateWithData(data as CFData, nil),
              let image = CGImageSourceCreateImageAtIndex(source, 0, [kCGImageSourceShouldCacheImmediately: true] as CFDictionary),
              CGImageSourceGetStatusAtIndex(source, 0) == .statusComplete,
              Double(image.width) == box[2] - box[0], Double(image.height) == box[3] - box[1] else { return nil }
        return image
    }

    var hasUsableErasePolygons: Bool {
        guard let masks = erase_points else { return true }  // Older flat fills use the region contour.
        return !masks.isEmpty && masks.allSatisfy { points in
            guard points.count >= 3, points.allSatisfy({ $0.count == 2 && $0.allSatisfy(\.isFinite) }) else { return false }
            var twiceArea = 0.0
            var previous = points[points.count - 1]
            for point in points {
                twiceArea += previous[0] * point[1] - point[0] * previous[1]
                previous = point
            }
            return twiceArea.isFinite && twiceArea != 0
        }
    }

    var rectangle: CGRect? {
        guard text_box.count == 4, text_box.allSatisfy(\.isFinite),
              text_box[2] > text_box[0], text_box[3] > text_box[1] else { return nil }
        return CGRect(x: text_box[0], y: text_box[1], width: text_box[2] - text_box[0], height: text_box[3] - text_box[1])
    }
}

enum ReadingTextDirection: String, Codable, CaseIterable {
    case automatic, horizontal, vertical
}

enum ReadingTextAlignment: String, Codable, CaseIterable {
    case leading, center, trailing
}

struct ReadingTextStyle: Codable, Equatable {
    var fontName: String? = nil
    var fontSize: Double? = nil
    var direction: ReadingTextDirection? = nil
    var lineSpacing: Double? = nil
    var alignment: ReadingTextAlignment? = nil

    static let fontSizeRange = 6.0...160.0
    static let lineSpacingRange = 1.0...3.0

    fileprivate var isValid: Bool {
        (fontSize.map(Self.fontSizeRange.contains) ?? true)
            && (lineSpacing.map(Self.lineSpacingRange.contains) ?? true)
    }

    fileprivate func face(size: Double) -> NSFont {
        if let fontName, !fontName.isEmpty,
           let face = NSFont(name: fontName, size: size)
               ?? NSFontManager.shared.font(withFamily: fontName, traits: [], weight: 5, size: size) {
            return face
        }
        return .systemFont(ofSize: size, weight: .medium)
    }
}

struct ReadingGlyph {
    var text: String
    var center: CGPoint
}

struct ReadingTypeset {
    var glyphs: [ReadingGlyph]
    var fontSize: Double
    var bounds: CGRect
    var fontName: String? = nil
    var direction: ReadingTextDirection = .horizontal
    var lineSpacing: Double = 1.38
    var alignment: ReadingTextAlignment = .center
    var sourcePixelScale: Double = 1
    var sourceFontSize: Double { fontSize / sourcePixelScale }
}

struct ReadingRun {
    var blockIDs: [String]
    var layout: ReadingTypeset
}

struct ReadingPresentation: Identifiable {
    var region: ReadingRegion
    var text: String
    var runs: [ReadingRun]
    var background: CGImage?
    var id: String { region.id }
    var layout: ReadingTypeset { runs[0].layout }

    func layout(for blockID: String) -> ReadingTypeset {
        runs.first { $0.blockIDs.contains(blockID) }?.layout ?? layout
    }
}

enum ReadingLayout {
    // All layout measurements stay in the page coordinate system, including for enhanced output.
    static func presentations(page: PageResult, regions: [ReadingRegion],
                              styles: [String: ReadingTextStyle] = [:], sourcePixelScale: Double = 1) -> [ReadingPresentation] {
        guard sourcePixelScale.isFinite, sourcePixelScale > 0 else { return [] }
        let preferred = max(18, min(36, Double(page.width) * 0.023))
        var used = Set<String>()
        return regions.compactMap { region in
            guard !region.block_ids.isEmpty, used.isDisjoint(with: region.block_ids),
                  Set(region.block_ids).count == region.block_ids.count, region.rectangle != nil,
                  region.points.count >= 3,
                  region.points.allSatisfy({ $0.count == 2 && $0.allSatisfy(\.isFinite) }) else { return nil }
            let members = region.block_ids.compactMap { id in page.blocks.first { $0.id == id } }
            guard members.count == region.block_ids.count, members.allSatisfy({
                !$0.keepsOriginal && !$0.needsOCRReview && !$0.translation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && $0.error.isEmpty
            }) else { return nil }
            let background = region.decodedBackground
            guard region.background_png == nil ? region.hasUsableErasePolygons : background != nil else { return nil }
            let text = members.map(\.translation).joined(separator: "\n")
            let sourceStyles = members.map { styles[$0.id] ?? ReadingTextStyle() }
            guard sourceStyles.allSatisfy(\.isValid) else { return nil }
            let styles = sourceStyles.map { sourceStyle in
                var style = sourceStyle
                style.fontSize = sourceStyle.fontSize.map { $0 * sourcePixelScale }
                // An explicit return to the system face or automatic direction can share the default flow.
                if style.fontName == "" { style.fontName = nil }
                if style.direction == .automatic { style.direction = nil }
                return style
            }
            var runs: [ReadingRun]
            if styles.dropFirst().allSatisfy({ $0 == styles[0] }) {
                guard let layout = arrange(text, region: region, preferred: preferred, style: styles[0]) else { return nil }
                runs = [ReadingRun(blockIDs: region.block_ids, layout: layout)]
            } else {
                guard let separate = separateRuns(members, styles: styles, region: region, preferred: preferred) else { return nil }
                runs = separate
            }
            for index in runs.indices { runs[index].layout.sourcePixelScale = sourcePixelScale }
            used.formUnion(region.block_ids)
            return ReadingPresentation(region: region, text: text, runs: runs, background: background)
        }
    }

    private static func arrange(_ text: String, region: ReadingRegion, preferred: Double, style: ReadingTextStyle,
                                slot: CGRect? = nil) -> ReadingTypeset? {
        let direction = style.direction ?? .automatic
        let paragraphStyle = region.overlay == "narration"
        if direction == .vertical {
            return flow(text, polygon: region.points, preferred: preferred, style: style, vertical: true,
                        paragraphStyle: paragraphStyle, slot: slot)
        }
        let across = flow(text, polygon: region.points, preferred: preferred, style: style, vertical: false,
                          paragraphStyle: paragraphStyle, slot: slot)
        guard direction == .automatic, region.vertical, let rect = region.rectangle else { return across }
        let box = slot.map { $0.intersection(rect) } ?? rect
        guard !box.isEmpty, box.height >= box.width * 3, box.width < preferred * 2.2,
              text.unicodeScalars.allSatisfy({ !$0.isASCII || CharacterSet.whitespacesAndNewlines.contains($0)
                  || CharacterSet.punctuationCharacters.contains($0) }) else { return across }
        let down: ReadingTypeset?
        if slot == nil && style.fontName == nil && style.fontSize == nil && style.lineSpacing == nil && style.alignment == nil {
            down = narrowVertical(text, region: region, preferred: preferred)
        } else {
            down = flow(text, polygon: region.points, preferred: preferred, style: style, vertical: true,
                        paragraphStyle: paragraphStyle, slot: box)
        }
        if let down, across.map({ down.fontSize >= $0.fontSize * 1.1 }) ?? true { return down }
        return across
    }

    // Mixed typography keeps each dialogue's settings and still paints the shared patch once.
    // Intersect each slot with the original polygon during flow, including concave regions.
    private static func separateRuns(_ blocks: [TextBlock], styles: [ReadingTextStyle], region: ReadingRegion,
                                     preferred: Double) -> [ReadingRun]? {
        let xs = region.points.map { $0[0] }, ys = region.points.map { $0[1] }
        let bounds = CGRect(x: xs.min()!, y: ys.min()!, width: xs.max()! - xs.min()!, height: ys.max()! - ys.min()!)
        guard !bounds.isEmpty else { return nil }
        let weights = zip(blocks, styles).map { block, style in
            Double(max(1, block.translation.count)) * pow(style.fontSize ?? preferred, 2)
        }
        let preferColumns = styles.allSatisfy { $0.direction == .vertical }
        for columns in [preferColumns, !preferColumns] {
            for shares in [weights, Array(repeating: 1.0, count: blocks.count)] {
                let total = shares.reduce(0, +)
                var start = 0.0
                var runs: [ReadingRun] = []
                for index in blocks.indices {
                    let end = start + shares[index] / total
                    let slot = columns
                        ? CGRect(x: bounds.maxX - end * bounds.width, y: bounds.minY,
                                 width: (end - start) * bounds.width, height: bounds.height)
                        : CGRect(x: bounds.minX, y: bounds.minY + start * bounds.height,
                                 width: bounds.width, height: (end - start) * bounds.height)
                    guard let layout = arrange(blocks[index].translation, region: region, preferred: preferred,
                                               style: styles[index], slot: slot) else { break }
                    runs.append(ReadingRun(blockIDs: [blocks[index].id], layout: layout))
                    start = end
                }
                if runs.count == blocks.count { return runs }
            }
        }
        return nil
    }

    // Marginal handwritten annotations can be only one character wide. Use
    // their original direction when horizontal layout fails or shrinks too far.
    // The entire glyph envelope must still stay inside the safe polygon.
    private static func narrowVertical(_ text: String, region: ReadingRegion, preferred: Double) -> ReadingTypeset? {
        guard let box = region.rectangle else { return nil }
        let path = CGMutablePath()
        path.move(to: CGPoint(x: region.points[0][0], y: region.points[0][1]))
        for point in region.points.dropFirst() { path.addLine(to: CGPoint(x: point[0], y: point[1])) }
        path.closeSubpath()
        let floor = max(12, preferred * 0.5)
        var size = min(preferred, box.width / 1.55)
        while size >= floor {
            if let layout = typeset(text, in: box, vertical: true, preferred: size), layout.fontSize >= floor {
                let fits = layout.glyphs.allSatisfy { glyph in
                    [-0.55, 0.55].allSatisfy { dx in
                        [-0.66, 0.66].allSatisfy { dy in
                            path.contains(CGPoint(x: glyph.center.x + dx * layout.fontSize,
                                                  y: glyph.center.y + dy * layout.fontSize))
                        }
                    }
                }
                if fits { return layout }
            }
            size -= 0.5
        }
        return nil
    }

    static func failureReason(blockID: String, page: PageResult, regions: [ReadingRegion]) -> String {
        guard let region = regions.first(where: { $0.block_ids.contains(blockID) }) else {
            return "未找到可安全回填的区域，已保留原图。"
        }
        let members = page.blocks.filter { region.block_ids.contains($0.id) }
        if members.count != region.block_ids.count || members.contains(where: { $0.translation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !$0.error.isEmpty || $0.needsOCRReview || $0.keepsOriginal }) {
            return "同框文字尚未全部就绪，已保留原图。请先完成或核对同框对白。"
        }
        if region.background_png == nil ? !region.hasUsableErasePolygons : region.decodedBackground == nil {
            return "背景修补数据无效，已保留原图。"
        }
        return "译文在当前区域内排不下，已保留原图。可缩小字号、调整间距或缩短译文。"
    }

    private static let opening = Set(Array("（([《〈「『【“‘"))
    private static let closing = Set(Array("，。、；：！？,.!?;:）)]》〉」』】”’…"))
    private static let verticalForms: [Character: String] = [
        "，": "︐", "、": "︑", "。": "︒", "：": "︓", "；": "︔", "！": "︕", "？": "︖",
        "（": "︵", "）": "︶", "《": "︽", "》": "︾", "「": "﹁", "」": "﹂", "『": "﹃", "』": "﹄", "…": "︙", "—": "︱"
    ]

    private struct Span { var left: Double; var right: Double }

    // Intersect scanline spans over the full height of the glyph cell, not only its center.
    // This lets long Chinese lines use the sloping sides of an irregular balloon safely.
    private static func rowSpans(polygon: [[Double]], y: Double, halfHeight: Double,
                                 margin: Double, checkVertices: Bool) -> [Span] {
        var common: [Span]?
        let samples: [Double]
        if checkVertices {
            // Between consecutive vertex heights polygon edges are straight. Check both
            // one-sided limits at a notch, even when it is narrower than a pixel.
            let vertices = Set(polygon.lazy.map { $0[1] }.filter { abs($0 - y) < halfHeight })
            samples = [y - halfHeight, y + halfHeight] + vertices.flatMap { [$0.nextDown, $0.nextUp] }
        } else {
            let steps = max(2, Int(ceil(halfHeight * 2 / 3)))
            samples = (0...steps).map { y - halfHeight + Double($0) * halfHeight * 2 / Double(steps) }
        }
        for scanY in samples {
            var intersections: [Double] = []
            var previous = polygon.last!
            for point in polygon {
                if (point[1] <= scanY && scanY < previous[1]) || (previous[1] <= scanY && scanY < point[1]) {
                    intersections.append(point[0] + (scanY - point[1]) * (previous[0] - point[0]) / (previous[1] - point[1]))
                }
                previous = point
            }
            intersections.sort()
            var spans: [Span] = []
            for i in stride(from: 0, to: intersections.count - 1, by: 2) {
                let span = Span(left: intersections[i] + margin, right: intersections[i + 1] - margin)
                if span.right > span.left { spans.append(span) }
            }
            if let old = common {
                common = old.flatMap { a in spans.compactMap { b -> Span? in
                    let left = max(a.left, b.left), right = min(a.right, b.right)
                    return right > left ? Span(left: left, right: right) : nil
                }}
            } else { common = spans }
            if common?.isEmpty == true { return [] }
        }
        return common ?? []
    }

    static func horizontal(_ text: String, polygon: [[Double]], preferred: Double,
                           paragraphStyle: Bool = false) -> ReadingTypeset? {
        flow(text, polygon: polygon, preferred: preferred, style: ReadingTextStyle(),
             vertical: false, paragraphStyle: paragraphStyle, slot: nil)
    }

    // SwiftUI centers each upright glyph on its typographic line. Include ink that
    // extends asymmetrically from that center, plus the outline drawn on a patch.
    // Font line leading is space between horizontal lines; repeating it after each
    // character in a vertical column needlessly pushes marginal notes out of bounds.
    private static func verticalInkCell(_ glyph: String, face: NSFont) -> CGSize {
        let attributed = NSAttributedString(string: glyph, attributes: [.font: face])
        let line = CTLineCreateWithAttributedString(attributed)
        var ascent: CGFloat = 0, descent: CGFloat = 0
        let width = CTLineGetTypographicBounds(line, &ascent, &descent, nil)
        let ink = CTLineGetImageBounds(line, nil)
        guard !ink.isNull else { return attributed.size() }
        let outline = max(0.5, face.pointSize * 0.055)
        let centerY = (ascent - descent) / 2
        return CGSize(width: 2 * (max(abs(ink.minX - width / 2), abs(ink.maxX - width / 2)) + outline),
                      height: 2 * (max(abs(ink.minY - centerY), abs(ink.maxY - centerY)) + outline))
    }

    // In flow coordinates u advances along a line. For vertical writing u = y and v = −x,
    // so wrapping moves a column from right to left using the same polygon intersections.
    private static func flow(_ text: String, polygon source: [[Double]], preferred: Double, style: ReadingTextStyle,
                             vertical: Bool, paragraphStyle: Bool, slot: CGRect?) -> ReadingTypeset? {
        guard preferred.isFinite, preferred > 0, source.count >= 3,
              source.allSatisfy({ $0.count == 2 && $0.allSatisfy(\.isFinite) }) else { return nil }
        let polygon = vertical ? source.map { [$0[1], -$0[0]] } : source
        let paragraphs = text.components(separatedBy: .newlines).map { Array($0.trimmingCharacters(in: .whitespaces)) }.filter { !$0.isEmpty }
        guard !paragraphs.isEmpty else { return nil }
        let display = paragraphs.map { chars in chars.map { char in
            vertical ? verticalForms[char] ?? String(char) : String(char)
        }}
        let alignment = style.alignment ?? (paragraphStyle && !vertical ? .leading : .center)
        let lineSpacing = style.lineSpacing ?? (vertical ? 1.24 : 1.38)
        let custom = vertical || slot != nil || style.fontName != nil || style.fontSize != nil
            || style.lineSpacing != nil || style.alignment != nil
        let wordBoundaries: [Set<Int>] = paragraphs.map { characters in
            let value = String(characters)
            let tokenizer = NLTokenizer(unit: .word)
            tokenizer.string = value
            tokenizer.setLanguage(.simplifiedChinese)
            var boundaries: Set<Int> = [0, characters.count]
            tokenizer.enumerateTokens(in: value.startIndex..<value.endIndex) { range, _ in
                boundaries.insert(value.distance(from: value.startIndex, to: range.lowerBound))
                boundaries.insert(value.distance(from: value.startIndex, to: range.upperBound))
                return true
            }
            return boundaries
        }
        let xs = polygon.map { $0[0] }, ys = polygon.map { $0[1] }
        var area = CGRect(x: xs.min()!, y: ys.min()!, width: xs.max()! - xs.min()!, height: ys.max()! - ys.min()!)
        if let slot {
            let limit = vertical ? CGRect(x: slot.minY, y: -slot.maxX, width: slot.height, height: slot.width) : slot
            area = area.intersection(limit)
        }
        guard !area.isEmpty else { return nil }
        let minY = area.minY, maxY = area.maxY, minX = area.minX, maxX = area.maxX
        var font = style.fontSize ?? preferred
        let minimum = style.fontSize ?? max(12, preferred * 0.50)
        while font >= minimum {
            let advance = font * 1.03, lineHeight = font * lineSpacing
            let face = style.face(size: font)
            let measured = display.map { line in line.map {
                vertical ? verticalInkCell($0, face: face) : ($0 as NSString).size(withAttributes: [.font: face])
            }}
            let widths = measured.map { line in line.map {
                vertical ? max(font * 1.12, $0.height) : max(font * 0.3, $0.width + font * 0.03)
            }}
            let halfHeight: Double
            if custom {
                let cross = measured.lazy.flatMap { $0 }.map { vertical ? $0.width : $0.height }.max() ?? font
                halfHeight = vertical ? max(font * 0.55, cross / 2)
                    : max(font * 0.66, cross / 2 + max(0.5, font * 0.055))
            } else { halfHeight = font * 0.66 }
            let margin = vertical ? max(2, font * 0.12) : max(4, font * 0.2)
            // Quantize candidate row centers, then reuse spans across every possible starting row.
            let step = lineHeight / 4
            let firstY = minY + halfHeight + margin
            let lastRow = floor((maxY - halfHeight - margin - firstY) / step)
            guard let lastIndex = Int(exactly: lastRow), lastIndex < Int.max else { return nil }
            if lastIndex < 0 { font -= 0.5; continue }
            let count = lastIndex + 1
            let rows = (0..<count).map { index in
                let spans = rowSpans(polygon: polygon, y: firstY + Double(index) * step,
                                     halfHeight: halfHeight, margin: margin, checkVertices: custom)
                guard slot != nil else { return spans }
                return spans.compactMap { span in
                    let clipped = Span(left: max(span.left, minX + margin), right: min(span.right, maxX - margin))
                    return clipped.right > clipped.left ? clipped : nil
                }
            }
            var best: ReadingTypeset?
            var bestScore = Double.greatestFiniteMagnitude
            for start in 0..<count {
                var paragraph = 0, offset = 0
                var glyphs: [ReadingGlyph] = []
                var shortLinePenalty = 0.0
                var rowIndex = start
                while rowIndex < count && paragraph < paragraphs.count {
                    let chars = paragraphs[paragraph]
                    let y = firstY + Double(rowIndex) * step
                    // A line uses one continuous span; it never bridges the outside of a concave shape.
                    let minimumColumns = vertical || chars.count <= 4 ? 1.0 :
                        (offset == 0 && maxX - minX >= advance * 4 ? 3.0 : min(2, Double(chars.count - offset)))
                    let spans = rows[rowIndex].filter { $0.right - $0.left >= advance * minimumColumns }
                    if let span = spans.max(by: { $0.right - $0.left < $1.right - $1.left }) {
                        var end = offset, occupied = 0.0
                        while end < chars.count && occupied + widths[paragraph][end] <= span.right - span.left {
                            occupied += widths[paragraph][end]; end += 1
                        }
                        guard end > offset else { rowIndex += 4; continue }
                        if end < chars.count {
                            if !wordBoundaries[paragraph].contains(end),
                               let boundary = wordBoundaries[paragraph].filter({ $0 > offset && $0 < end && end - $0 <= 2 }).max() {
                                end = boundary
                            }
                            while end > offset + 1 && (closing.contains(chars[end]) || opening.contains(chars[end - 1])) { end -= 1 }
                            // Punctuation backtracking can itself cut a Chinese word; recheck that boundary.
                            if !wordBoundaries[paragraph].contains(end),
                               let boundary = wordBoundaries[paragraph].filter({ $0 > offset && $0 < end && end - $0 <= 2 }).max(),
                               !closing.contains(chars[boundary]), !opening.contains(chars[boundary - 1]) { end = boundary }
                        }
                        let length = end - offset
                        if chars.count > 7 && length < 3 { shortLinePenalty += Double(3 - length) * font * 0.7 }
                        if length > 0 {
                            let runWidth = widths[paragraph][offset..<end].reduce(0, +)
                            var cursor: Double
                            switch alignment {
                            case .leading: cursor = span.left
                            case .center: cursor = (span.left + span.right - runWidth) / 2
                            case .trailing: cursor = span.right - runWidth
                            }
                            for index in offset..<end {
                                let width = widths[paragraph][index]
                                glyphs.append(ReadingGlyph(text: display[paragraph][index], center: CGPoint(x: cursor + width / 2, y: y)))
                                cursor += width
                            }
                            offset = end
                            if offset == chars.count { paragraph += 1; offset = 0 }
                        }
                    }
                    rowIndex += 4
                }
                guard paragraph == paragraphs.count, let top = glyphs.first?.center.y, let bottom = glyphs.last?.center.y else { continue }
                let left = glyphs.map { $0.center.x }.min()! - font / 2
                let right = glyphs.map { $0.center.x }.max()! + font / 2
                let bounds = CGRect(x: left, y: top - halfHeight, width: right - left, height: bottom - top + halfHeight * 2)
                let centerPenalty = abs(bounds.midY - (minY + maxY) / 2)
                let score = centerPenalty + bounds.height * (paragraphStyle ? 0.02 : 0.06) + shortLinePenalty
                if score < bestScore {
                    bestScore = score
                    let resolvedName = style.fontName != nil && face.fontName != NSFont.systemFont(ofSize: font, weight: .medium).fontName
                        ? face.fontName : nil
                    let outputGlyphs = vertical ? glyphs.map {
                        ReadingGlyph(text: $0.text, center: CGPoint(x: -$0.center.y, y: $0.center.x))
                    } : glyphs
                    let outputBounds = vertical ? CGRect(x: -bounds.maxY, y: bounds.minX, width: bounds.height, height: bounds.width) : bounds
                    best = ReadingTypeset(glyphs: outputGlyphs, fontSize: font, bounds: outputBounds,
                                          fontName: resolvedName, direction: vertical ? .vertical : .horizontal,
                                          lineSpacing: lineSpacing, alignment: alignment)
                }
            }
            if let best { return best }
            font -= 0.5
        }
        return nil
    }

    // Deterministic cells prevent SwiftUI's horizontal wrapping/shrinking from masquerading as vertical text.
    // Return nil when the full translation cannot fit legibly; the original stays visible and the inspector has the full text.
    static func typeset(_ text: String, in box: CGRect, vertical: Bool, preferred: Double) -> ReadingTypeset? {
        guard preferred.isFinite, preferred > 0, box.width > 0, box.height > 0 else { return nil }
        let paragraphs = text.components(separatedBy: .newlines).map {
            Array($0).filter { vertical ? !$0.isWhitespace : true }
        }.filter { !$0.isEmpty }
        guard !paragraphs.isEmpty else { return nil }
        let padding = max(2, preferred * 0.12)
        let area = box.insetBy(dx: padding, dy: padding)
        guard area.width > 0, area.height > 0 else { return nil }
        var font = preferred
        while font >= preferred * 0.72 {
            let advance = font * 1.12
            let cross = font * 1.24
            let capacity = Int(floor((vertical ? area.height : area.width) / advance))
            let maxLines = Int(floor((vertical ? area.width : area.height) / cross))
            if capacity > 0 && maxLines > 0 {
                var lines: [[Character]] = []
                for chars in paragraphs {
                    var start = 0
                    while start < chars.count {
                        var end = min(start + capacity, chars.count)
                        if end < chars.count {
                            // Keep closing punctuation off the beginning and opening punctuation off the end.
                            while end > start + 1 && (closing.contains(chars[end]) || opening.contains(chars[end - 1])) { end -= 1 }
                        }
                        lines.append(Array(chars[start..<end])); start = end
                    }
                }
                if lines.count <= maxLines {
                    let lineExtent = Double(lines.count) * cross
                    let runExtent = Double(lines.map(\.count).max() ?? 0) * advance
                    let bounds = CGRect(x: area.midX - (vertical ? lineExtent : runExtent) / 2,
                                        y: area.midY - (vertical ? runExtent : lineExtent) / 2,
                                        width: vertical ? lineExtent : runExtent,
                                        height: vertical ? runExtent : lineExtent)
                    var glyphs: [ReadingGlyph] = []
                    for (line, chars) in lines.enumerated() {
                        for (index, char) in chars.enumerated() {
                            let x = vertical ? bounds.maxX - (Double(line) + 0.5) * cross : area.midX + (Double(index) + 0.5 - Double(chars.count) / 2) * advance
                            let y = vertical ? bounds.minY + (Double(index) + 0.5) * advance : bounds.minY + (Double(line) + 0.5) * cross
                            glyphs.append(ReadingGlyph(text: vertical ? verticalForms[char] ?? String(char) : String(char), center: CGPoint(x: x, y: y)))
                        }
                    }
                    return ReadingTypeset(glyphs: glyphs, fontSize: font, bounds: bounds,
                                          direction: vertical ? .vertical : .horizontal, lineSpacing: 1.24,
                                          alignment: vertical ? .leading : .center)
                }
            }
            font -= 0.5
        }
        return nil
    }
}

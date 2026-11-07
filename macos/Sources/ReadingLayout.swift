import Foundation
import CoreGraphics
import NaturalLanguage

struct TextBlock: Codable, Identifiable {
    var id: String
    var box: [Double]
    var source: String
    var translation: String
    var error: String
    var vertical: Bool
    var edited: Bool
    var vision_source: String?
    var quality_warnings: [String]?
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

    var rectangle: CGRect? {
        guard text_box.count == 4, text_box.allSatisfy(\.isFinite),
              text_box[2] > text_box[0], text_box[3] > text_box[1] else { return nil }
        return CGRect(x: text_box[0], y: text_box[1], width: text_box[2] - text_box[0], height: text_box[3] - text_box[1])
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
}

struct ReadingPresentation: Identifiable {
    var region: ReadingRegion
    var text: String
    var layout: ReadingTypeset
    var id: String { region.id }
}

enum ReadingLayout {
    // Use page pixels, not zoom or OCR-column width, to keep adjacent bubbles consistent.
    static func presentations(page: PageResult, regions: [ReadingRegion]) -> [ReadingPresentation] {
        let preferred = max(18, min(36, Double(page.width) * 0.023))
        var used = Set<String>()
        return regions.compactMap { region in
            guard !region.block_ids.isEmpty, used.isDisjoint(with: region.block_ids),
                  region.rectangle != nil else { return nil }
            let members = region.block_ids.compactMap { id in page.blocks.first { $0.id == id } }
            guard members.count == region.block_ids.count,
                  members.allSatisfy({ !$0.translation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && $0.error.isEmpty }) else { return nil }
            if let encoded = region.background_png {
                guard let box = region.background_box, box.count == 4, box.allSatisfy(\.isFinite),
                      box[2] > box[0], box[3] > box[1], let imageData = Data(base64Encoded: encoded),
                      imageData.starts(with: [137, 80, 78, 71, 13, 10, 26, 10]) else { return nil }
            }
            let text = members.map(\.translation).joined(separator: "\n")
            let layout = horizontal(text, polygon: region.points, preferred: preferred,
                                    paragraphStyle: region.overlay == "narration")
            guard let layout else { return nil }
            used.formUnion(region.block_ids)
            return ReadingPresentation(region: region, text: text, layout: layout)
        }
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
                                 margin: Double) -> [Span] {
        var common: [Span]?
        let steps = max(2, Int(ceil(halfHeight * 2 / 3)))
        for sample in 0...steps {
            let scanY = y - halfHeight + Double(sample) * halfHeight * 2 / Double(steps)
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
        guard preferred.isFinite, preferred > 0, polygon.count >= 3,
              polygon.allSatisfy({ $0.count == 2 && $0.allSatisfy(\.isFinite) }) else { return nil }
        let paragraphs = text.components(separatedBy: .newlines).map { Array($0.trimmingCharacters(in: .whitespaces)) }.filter { !$0.isEmpty }
        guard !paragraphs.isEmpty else { return nil }
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
        let minY = polygon.map { $0[1] }.min()!, maxY = polygon.map { $0[1] }.max()!
        let minX = polygon.map { $0[0] }.min()!, maxX = polygon.map { $0[0] }.max()!
        guard maxY > minY, maxX > minX else { return nil }
        var font = preferred
        // Keep a modest font floor. Full shape flow generally fits more text than one inscribed rectangle.
        while font >= preferred * 0.72 {
            let advance = font * 1.03, lineHeight = font * 1.38
            let halfHeight = font * 0.66, margin = max(4, font * 0.2)
            // Quantize candidate row centers, then reuse spans across every possible starting row.
            let step = lineHeight / 4
            let firstY = minY + halfHeight + margin
            let count = max(0, Int(floor((maxY - halfHeight - margin - firstY) / step)) + 1)
            if count == 0 { font -= 0.5; continue }
            let rows = (0..<count).map { rowSpans(polygon: polygon, y: firstY + Double($0) * step, halfHeight: halfHeight, margin: margin) }
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
                    let minimumColumns = chars.count <= 4 ? 1.0 :
                        (offset == 0 && maxX - minX >= advance * 4 ? 3.0 : min(2, Double(chars.count - offset)))
                    let spans = rows[rowIndex].filter { $0.right - $0.left >= advance * minimumColumns }
                    if let span = spans.max(by: { $0.right - $0.left < $1.right - $1.left }) {
                        let capacity = Int(floor((span.right - span.left) / advance))
                        var end = min(offset + capacity, chars.count)
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
                            let left = paragraphStyle ? span.left : (span.left + span.right - Double(length) * advance) / 2
                            for index in offset..<end {
                                glyphs.append(ReadingGlyph(text: String(chars[index]), center: CGPoint(x: left + (Double(index - offset) + 0.5) * advance, y: y)))
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
                    best = ReadingTypeset(glyphs: glyphs, fontSize: font, bounds: bounds)
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
                    return ReadingTypeset(glyphs: glyphs, fontSize: font, bounds: bounds)
                }
            }
            font -= 0.5
        }
        return nil
    }
}

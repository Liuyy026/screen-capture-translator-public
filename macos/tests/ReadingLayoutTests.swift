import Foundation
import CoreGraphics

@main struct ReadingLayoutTests {
    static func main() throws {
        let rect = CGRect(x: 10, y: 20, width: 100, height: 170)
        let text = "从刚才起就让人火大呢"
        let layout = ReadingLayout.typeset(text, in: rect, vertical: true, preferred: 28)!
        precondition(layout.glyphs.map(\.text).joined() == text, "Characters must appear once in reading order")
        precondition(layout.glyphs[1].center.y > layout.glyphs[0].center.y)
        let columns = Set(layout.glyphs.map { $0.center.x })
        precondition(columns.count > 1, "Must wrap vertically, not shrink to a single tiny column")
        let firstLeft = layout.glyphs.firstIndex { $0.center.x < layout.glyphs[0].center.x }!
        precondition(layout.glyphs[firstLeft].center.y < layout.glyphs[firstLeft - 1].center.y)
        precondition(layout.fontSize >= 28 * 0.72)
        for glyph in layout.glyphs { precondition(rect.contains(glyph.center)) }
        precondition(ReadingLayout.typeset(String(repeating: "很长", count: 300), in: rect, vertical: true, preferred: 28) == nil)
        precondition(ReadingLayout.typeset("", in: rect, vertical: true, preferred: 28) == nil)
        let horizontal = ReadingLayout.typeset("这是横排，完整显示。", in: CGRect(x: 0, y: 0, width: 160, height: 100), vertical: false, preferred: 28)!
        precondition(horizontal.glyphs.count == 10)
        precondition(horizontal.glyphs[1].center.x > horizontal.glyphs[0].center.x)
        let punctuation = ReadingLayout.typeset("你好，世界！", in: rect, vertical: true, preferred: 28)!
        precondition(punctuation.glyphs.map(\.text).contains("︐"))
        let multiline = ReadingLayout.typeset("甲乙\n丙丁", in: rect, vertical: true, preferred: 28)!
        precondition(multiline.glyphs.count == 4)
        precondition(multiline.glyphs[2].center.x < multiline.glyphs[0].center.x)
        var region = ReadingRegion(id: "r", block_ids: ["1", "2"], points: [[0,0],[100,0],[100,200],[0,200]],
                                   text_box: [0,0,100,200], fill_color: [255,255,255], vertical: true)
        var page = PageResult(version: 2, width: 1280, height: 1775, blocks: [
            TextBlock(id: "1", box: [0,0,30,100], source: "原文", translation: "已经翻译", error: "", vertical: true, edited: false),
            TextBlock(id: "2", box: [40,0,70,100], source: "原文", translation: "", error: "", vertical: true, edited: false)
        ], model: "test", ocr: "test")
        precondition(ReadingLayout.presentations(page: page, regions: [region]).isEmpty, "Never erase unready group member")
        page.blocks[1].translation = "完成"
        precondition(ReadingLayout.presentations(page: page, regions: [region]).count == 1)
        page.blocks[1].error = "失败"
        precondition(ReadingLayout.presentations(page: page, regions: [region]).isEmpty)
        page.blocks[1].error = ""
        region.block_ids = ["missing"]
        precondition(ReadingLayout.presentations(page: page, regions: [region]).isEmpty)
        region.text_box = [1,2]
        precondition(region.rectangle == nil)
        let oval = (0..<120).map { index -> [Double] in
            let angle = Double(index) * 2 * .pi / 120
            return [140 + 105 * cos(angle), 220 + 180 * sin(angle)]
        }
        let longText = "但有一件事我绝不允许被否定——那就是曾有同伴为大义而殉难！！"
        let flowing = ReadingLayout.horizontal(longText, polygon: oval, preferred: 29.44)!
        precondition(flowing.glyphs.map(\.text).joined() == longText)
        precondition(flowing.glyphs[1].center.x > flowing.glyphs[0].center.x, "Chinese defaults to left-to-right")
        for glyph in flowing.glyphs {
            for dx in [-0.52, 0.52] { for dy in [-0.65, 0.65] {
                let x = (glyph.center.x + dx * flowing.fontSize - 140) / 105
                let y = (glyph.center.y + dy * flowing.fontSize - 220) / 180
                precondition(x*x + y*y < 1, "Full glyph envelope stays inside balloon")
            }}
        }
        let narrow = ReadingLayout.horizontal("我是", polygon: [[0,0],[46,0],[46,100],[0,100]], preferred: 29.44)
        precondition(narrow?.glyphs.count == 2, "Tiny short bubble must remain readable")
        precondition(ReadingLayout.horizontal(String(repeating: "长句", count: 300), polygon: oval, preferred: 29.44) == nil)
        let withParagraphs = ReadingLayout.horizontal("那一天，我们输了\n胜者即为正义", polygon: oval, preferred: 29.44)!
        precondition(withParagraphs.glyphs.map(\.text).joined() == "那一天，我们输了胜者即为正义")
        precondition(withParagraphs.glyphs[8].center.y > withParagraphs.glyphs[7].center.y)
        let concave: [[Double]] = [[0,0],[105,0],[105,80],[180,80],[180,280],[0,280]]
        let shaped = ReadingLayout.horizontal("这是需要完整显示的长句，不能越过异形气泡的凹陷边界。", polygon: concave, preferred: 28)!
        let path = CGMutablePath()
        path.move(to: CGPoint(x: 0, y: 0))
        for point in concave.dropFirst() { path.addLine(to: CGPoint(x: point[0], y: point[1])) }
        path.closeSubpath()
        for glyph in shaped.glyphs {
            for dx in [-0.55, 0.55] { for dy in [-0.66, 0.66] {
                precondition(path.contains(CGPoint(x: glyph.center.x + dx * shaped.fontSize,
                                                   y: glyph.center.y + dy * shaped.fontSize)))
            }}
        }
        region.block_ids = ["1", "2"]
        region.text_box = [0,0,100,200]
        region.background_png = "invalid PNG"
        region.background_box = [0,0,100,200]
        precondition(ReadingLayout.presentations(page: page, regions: [region]).isEmpty,
                     "Invalid background must not draw Chinese over uncleared source")
        print("Reading layout checks passed: vertical order, wrapping, punctuation, complete text, legibility fallback and incomplete-group preservation")
    }
}

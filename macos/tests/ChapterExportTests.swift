import AppKit
import ImageIO
import UniformTypeIdentifiers

@main struct ChapterExportTests {
    @MainActor static func main() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: directory) }
        let source = directory.appendingPathComponent("p3-source.png")
        let output = directory.appendingPathComponent("export.png")
        let colorSpace = CGColorSpace(name: CGColorSpace.displayP3)!
        let width = 300, height = 500
        let bitmap = bitmap(width: width, height: height, colorSpace: colorSpace)
        bitmap.setFillColor(CGColor(colorSpace: colorSpace, components: [1, 0.2, 0, 1])!)
        bitmap.fill(CGRect(x: 0, y: 0, width: width, height: height))
        bitmap.setFillColor(CGColor(colorSpace: colorSpace, components: [0, 0.9, 0.25, 1])!)
        bitmap.fill(CGRect(x: 0, y: 0, width: width, height: height / 2))
        let destination = CGImageDestinationCreateWithURL(source as CFURL, UTType.png.identifier as CFString, 1, nil)!
        CGImageDestinationAddImage(destination, bitmap.makeImage()!, nil)
        precondition(CGImageDestinationFinalize(destination))

        let translated = TextBlock(id: "translated", box: [20, 15, 90, 55], source: "原文", translation: "翻译",
                                   error: "", vertical: false, edited: true)
        let kept = TextBlock(id: "kept", box: [25, 70, 95, 110], source: "原文", translation: "旧译文",
                              error: "", vertical: false, edited: true, keep_original: true)
        let untranslated = TextBlock(id: "untranslated", box: [105, 15, 145, 55], source: "原文", translation: "",
                                      error: "", vertical: false, edited: false)
        let page = PageResult(version: 2, width: 150, height: 125, blocks: [translated, kept, untranslated], model: "", ocr: "")
        let reply = ReadingOverlayService.Reply(version: 3, regions: [region(for: translated), region(for: kept)])
        let omitted = try await ChapterExport.write(.init(page: page, reply: reply, effective_image: source.path), to: output)

        let original = read(source), exported = read(output)
        precondition(exported.width == width && exported.height == height)
        precondition(exported.colorSpace?.name == colorSpace.name)
        precondition(omitted == 1)
        let before = pixels(original, colorSpace: colorSpace), after = pixels(exported, colorSpace: colorSpace)
        for y in 0..<height {
            for x in 0..<width where !(40..<180 ~= x && 60..<220 ~= y) {
                let offset = (y * width + x) * 4
                precondition(before[offset..<(offset + 4)] == after[offset..<(offset + 4)], "Pixel outside translated bubble changed at \(x), \(y)")
            }
        }
        let fillOffset = (65 * width + 45) * 4
        precondition(Array(after[fillOffset..<(fillOffset + 4)]) == [255, 255, 255, 255], "Translated overlay is missing or misaligned")
        let textPixel = (60..<220).contains { y in
            (40..<180).contains { x in after[(y * width + x) * 4] < 32 }
        }
        precondition(textPixel, "The translation glyphs are missing")
        print("Chapter export preserves Display P3 pixels outside the translated bubble and draws the translation at the exported size.")
    }

    private static func region(for block: TextBlock) -> ReadingRegion {
        let b = block.box
        return ReadingRegion(id: block.id, block_ids: [block.id], points: [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]],
                             text_box: b, fill_color: [255, 255, 255], vertical: false)
    }

    private static func read(_ url: URL) -> CGImage {
        CGImageSourceCreateImageAtIndex(CGImageSourceCreateWithURL(url as CFURL, nil)!, 0, nil)!
    }

    private static func bitmap(width: Int, height: Int, colorSpace: CGColorSpace) -> CGContext {
        CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                  space: colorSpace, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    }

    private static func pixels(_ image: CGImage, colorSpace: CGColorSpace) -> [UInt8] {
        let canvas = bitmap(width: image.width, height: image.height, colorSpace: colorSpace)
        canvas.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
        let count = image.width * image.height * 4
        return Array(UnsafeBufferPointer(start: canvas.data!.assumingMemoryBound(to: UInt8.self), count: count))
    }
}

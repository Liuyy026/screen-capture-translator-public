import Foundation
import ImageIO

// The sidecar travels with the chapter. It contains typography only; opening
// an unstyled chapter does not create a file or touch the recognition cache.
struct ChapterTypography: Codable {
    private var version = 1
    var defaults = ReadingTextStyle()
    private var pages: [String: [String: ReadingTextStyle]] = [:]
    static let filename = ".koma-typesetting.json"

    // Legacy OCR can use an enlarged canvas. User-entered sizes always refer
    // to the original comic, independently of that canvas or the output PNG.
    static func originalWidth(at image: URL) -> Double? {
        guard let source = CGImageSourceCreateWithURL(image as CFURL, nil),
              let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any] else { return nil }
        let rotated = (5...8).contains(properties[kCGImagePropertyOrientation] as? Int ?? 1)
        guard let pixels = properties[rotated ? kCGImagePropertyPixelHeight : kCGImagePropertyPixelWidth] as? Int,
              pixels > 0 else { return nil }
        return Double(pixels)
    }

    static func pixelScale(for page: PageResult, originalWidth: Double?) -> Double {
        guard let originalWidth, originalWidth > 0, page.width > 0 else { return 1 }
        return Double(page.width) / originalWidth
    }

    private static func key(for block: TextBlock, on page: PageResult) -> String? {
        guard page.width > 0, page.height > 0, block.box.count == 4,
              block.box.allSatisfy(\.isFinite) else { return nil }
        // OCR can reuse an ID for a different balloon. Match its geometry
        // as well, normalized so an equivalent OCR canvas has the same key.
        let bounds = block.box.enumerated().map { index, coordinate in
            let dimension = index.isMultiple(of: 2) ? page.width : page.height
            return String(format: "%.4f", locale: Locale(identifier: "en_US_POSIX"),
                          coordinate / Double(dimension))
        }
        return ([block.id] + bounds).joined(separator: ":")
    }

    static func pageKey(image: URL, cache: URL) -> String {
        image.lastPathComponent + "/" + cache.deletingPathExtension().lastPathComponent
    }

    func override(for block: TextBlock, on page: PageResult, key: String) -> ReadingTextStyle {
        Self.key(for: block, on: page).flatMap { pages[key]?[$0] } ?? ReadingTextStyle()
    }

    func resolved(for block: TextBlock, on page: PageResult, key: String) -> ReadingTextStyle {
        let local = override(for: block, on: page, key: key)
        return ReadingTextStyle(fontName: local.fontName ?? defaults.fontName,
                                fontSize: local.fontSize ?? defaults.fontSize,
                                direction: local.direction ?? defaults.direction,
                                lineSpacing: local.lineSpacing ?? defaults.lineSpacing,
                                alignment: local.alignment ?? defaults.alignment)
    }

    func styles(for page: PageResult, key: String) -> [String: ReadingTextStyle] {
        guard defaults != ReadingTextStyle() || pages[key] != nil else { return [:] }
        var result: [String: ReadingTextStyle] = [:]
        for block in page.blocks {
            let style = resolved(for: block, on: page, key: key)
            if style != ReadingTextStyle() { result[block.id] = style }
        }
        return result
    }

    mutating func set(_ style: ReadingTextStyle, for block: TextBlock, on page: PageResult, key: String) {
        guard let id = Self.key(for: block, on: page) else { return }
        if style == ReadingTextStyle() {
            pages[key]?[id] = nil
            if pages[key]?.isEmpty == true { pages[key] = nil }
        } else {
            pages[key, default: [:]][id] = style
        }
    }

    static func read(from url: URL) throws -> Self {
        let data: Data
        do { data = try Data(contentsOf: url) }
        catch let error as CocoaError where error.code == .fileReadNoSuchFile { return Self() }
        let value = try JSONDecoder().decode(Self.self, from: data)
        guard value.version == 1 else {
            throw NSError(domain: "Koma.ChapterTypography", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "此版本无法读取第 \(value.version) 版排版文件。"])
        }
        return value
    }

    // Merge with the current file under coordination, so another reader
    // editing a different bubble cannot lose its changes on our next save.
    static func update(at url: URL, change: (inout Self) -> Void) throws -> Self {
        var coordinationError: NSError?
        var saved: Result<Self, Error>?
        NSFileCoordinator().coordinate(writingItemAt: url, options: [], error: &coordinationError) { target in
            saved = Result {
                var value = try read(from: target)
                change(&value)
                let encoder = JSONEncoder()
                encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
                try encoder.encode(value).write(to: target, options: .atomic)
                return value
            }
        }
        if let coordinationError { throw coordinationError }
        guard let saved else { throw CocoaError(.fileWriteUnknown) }
        return try saved.get()
    }
}

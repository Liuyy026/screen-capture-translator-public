import AppKit
import CryptoKit
import Darwin
import ImageIO
import SwiftUI
import UniformTypeIdentifiers

enum ChapterExport {
    struct Options {
        let pages: IndexSet
        let folderName: String
        let directory: URL
    }

    struct Enhancement {
        let engine: String
        let target: Int
        let axis: String
        let preset: String

        var preferOriginal: Bool { engine == "off" }
        var settings: [String: Any] {
            ["engine": preferOriginal ? "metalfx" : engine, "target": target, "axis": axis,
             "preset": preferOriginal ? "standard" : preset]
        }
    }

    enum Outcome {
        case completed(directory: URL, omissions: [Int: Int])
        case cancelled
        case changed(page: Int)
        case failed(String)
    }

    struct Page: Decodable {
        let page: PageResult
        let reply: ReadingOverlayService.Reply
        let effective_image: String
        var cache: String? = nil
    }

    private struct LocalSettings: Decodable { let directory: String }
    private static let directoryPreference = "chapterExportFolder"

    static func defaultDirectory(project: URL, defaults: UserDefaults) -> URL {
        if let saved = defaults.url(forKey: directoryPreference), saved.isFileURL {
            return saved.standardizedFileURL
        }
        let config = project.appendingPathComponent(".runtime/chapter-export.json")
        if let data = try? Data(contentsOf: config),
           let settings = try? JSONDecoder().decode(LocalSettings.self, from: data),
           settings.directory.hasPrefix("/"), !settings.directory.utf8.contains(0) {
            return URL(fileURLWithPath: settings.directory, isDirectory: true).standardizedFileURL
        }
        return .documentsDirectory
    }

    static func rememberDirectory(_ directory: URL, defaults: UserDefaults) {
        defaults.set(directory, forKey: directoryPreference)
    }

    static func selectedPages(_ input: String, pageCount: Int) throws -> IndexSet {
        let formatError = NativeCoreService.Failure(message: "请输入页码，例如 1、3、5–8。")
        let boundsError = NativeCoreService.Failure(message: "页码须在 1 到 \(pageCount) 之间。")
        let text = input.precomposedStringWithCompatibilityMapping
        let entries = text.split(omittingEmptySubsequences: false) { ",，、".contains($0) }

        func pageNumber(_ part: Substring) throws -> Int {
            let digits = part.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !digits.isEmpty, digits.utf8.allSatisfy({ (48...57).contains($0) }) else { throw formatError }
            guard let page = Int(digits), page >= 1, page <= pageCount, page < Int.max else { throw boundsError }
            return page
        }

        var pages = IndexSet()
        for entry in entries {
            let bounds = entry.split(omittingEmptySubsequences: false) { "-–—−－".contains($0) }
            guard bounds.count == 1 || bounds.count == 2 else { throw formatError }
            let start = try pageNumber(bounds[0])
            let end = bounds.count == 2 ? try pageNumber(bounds[1]) : start
            guard start <= end else {
                throw NativeCoreService.Failure(message: "范围的起始页不能大于结束页。")
            }
            pages.insert(integersIn: start..<(end + 1))
        }
        guard !pages.isEmpty else { throw formatError }
        return pages
    }

    static func validatedFolderName(_ raw: String) throws -> String {
        let name = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else {
            throw NativeCoreService.Failure(message: "请输入导出文件夹名称。")
        }
        let forbidden = CharacterSet(charactersIn: "/:").union(.newlines)
        guard name != ".", name != "..",
              !name.unicodeScalars.contains(where: { forbidden.contains($0) || $0.properties.generalCategory == .control }) else {
            throw NativeCoreService.Failure(message: "请输入有效的文件夹名称，名称不能是 . 或 ..，也不能含有斜杠、冒号或换行等控制字符。")
        }
        guard name.utf16.count <= NAME_MAX else {
            throw NativeCoreService.Failure(message: "文件夹名称太长，请缩短后再导出。")
        }
        return name
    }

    static func stage(in directory: URL) throws -> URL {
        let manager = FileManager.default
        try manager.createDirectory(at: directory, withIntermediateDirectories: true)
        let staging = directory.appendingPathComponent(".koma-export-" + UUID().uuidString, isDirectory: true)
        try manager.createDirectory(at: staging, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        return staging
    }

    static func finish(_ staging: URL, in directory: URL, folderName: String) throws -> URL {
        let name = try validatedFolderName(folderName)
        var index = 1
        while true {
            let suffix = index == 1 ? "" : " (\(index))"
            var stem = name
            while stem.utf16.count + suffix.utf16.count > NAME_MAX { stem.removeLast() }
            let destination = directory.appendingPathComponent(stem + suffix, isDirectory: true)
            // Reserve the final name as part of the rename so simultaneous exports cannot replace one another.
            let result = renamex_np(staging.path, destination.path, UInt32(RENAME_EXCL))
            if result == 0 { return destination }
            let code = errno
            if code == ENOTSUP {
                do {
                    try FileManager.default.moveItem(at: staging, to: destination)
                    return destination
                } catch let error as CocoaError where error.code == .fileWriteFileExists { }
            } else if code != EEXIST {
                throw NSError(domain: NSPOSIXErrorDomain, code: Int(code), userInfo: [NSFilePathErrorKey: destination.path])
            }
            index += 1
        }
    }

    @MainActor static func write(_ input: Page, to url: URL, styles: [String: ReadingTextStyle] = [:],
                                 sourcePixelScale: Double = 1, sourceYScale: Double? = nil,
                                 expectedOriginalDigest: String? = nil) async throws -> Int {
        let page = input.page
        guard input.reply.version == 3, page.width > 0, page.height > 0 else {
            throw CocoaError(.fileReadCorruptFile)
        }
        let imageURL = URL(fileURLWithPath: input.effective_image)
        let base = try await Task.detached(priority: .userInitiated) {
            try decodeImage(at: imageURL, expectedDigest: expectedOriginalDigest)
        }.value
        try Task.checkCancellation()
        // When exporting the original, typography also uses the verified, EXIF-oriented pixels.
        let scale = expectedOriginalDigest == nil ? sourcePixelScale : Double(page.width) / Double(base.width)
        let yScale = expectedOriginalDigest == nil ? sourceYScale : Double(page.height) / Double(base.height)
        let presentations = ReadingLayout.presentations(page: page, regions: input.reply.regions,
                                                        styles: styles, sourcePixelScale: scale, sourceYScale: yScale)
        let shown = Set(presentations.flatMap(\.region.block_ids))
        let omitted = page.blocks.filter { block in
            if !block.keepsOriginal && !shown.contains(block.id)
                && (block.source_translation_pending == true || (block.ocr_confirmed == true && input.reply.preserved?[block.id] != nil)) {
                return true
            }
            return !block.keepsOriginal && !block.needsOCRReview && block.overlay != "skip" && block.text_kind != "noise"
                && !shown.contains(block.id) && input.reply.preserved?[block.id] == nil
                && (block.needsReadingOverlay || !block.error.isEmpty
                    || block.translation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }.count

        let size = CGSize(width: base.width, height: base.height)
        let layer = presentations.isEmpty ? nil : try renderOverlay(size: size, page: page, presentations: presentations)
        try await Task.detached(priority: .userInitiated) {
            let output = try layer.map { try composite($0, over: base) } ?? base
            guard let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else {
                throw CocoaError(.fileWriteUnknown)
            }
            CGImageDestinationAddImage(destination, output, nil)
            guard CGImageDestinationFinalize(destination) else { throw CocoaError(.fileWriteUnknown) }
        }.value
        return omitted
    }

    private static func decodeImage(at url: URL, expectedDigest: String?) throws -> CGImage {
        let source: CGImageSource?
        if let expectedDigest {
            // Hash and decode the same owned bytes even if the path is replaced after Rust replies.
            let changed = NativeCoreService.Failure(message: "导出时原图已变化，请重新核对。", code: "page_revision_changed")
            let handle: FileHandle
            do { handle = try FileHandle(forReadingFrom: url) }
            catch let error as CocoaError where error.code == .fileReadNoSuchFile || error.code == .fileNoSuchFile {
                throw changed
            } catch let error as POSIXError where error.code == .ENOENT {
                throw changed
            }
            defer { try? handle.close() }
            let bytes = try handle.readToEnd() ?? Data()
            let digest = SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined()
            guard digest == expectedDigest else { throw changed }
            source = CGImageSourceCreateWithData(bytes as CFData, nil)
        } else {
            source = CGImageSourceCreateWithURL(url as CFURL, nil)
        }
        let decodeOptions = [kCGImageSourceShouldCacheImmediately: true] as CFDictionary
        guard let source, let pixels = CGImageSourceCreateImageAtIndex(source, 0, decodeOptions) else {
            throw CocoaError(.fileReadCorruptFile)
        }
        let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any]
        guard let orientation = properties?[kCGImagePropertyOrientation] as? Int, orientation != 1 else { return pixels }
        let options = [kCGImageSourceCreateThumbnailFromImageAlways: true,
                       kCGImageSourceCreateThumbnailWithTransform: true,
                       kCGImageSourceShouldCacheImmediately: true,
                       kCGImageSourceThumbnailMaxPixelSize: max(pixels.width, pixels.height)] as [CFString: Any]
        guard let oriented = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else {
            throw CocoaError(.fileReadCorruptFile)
        }
        return oriented
    }

    @MainActor private static func renderOverlay(size: CGSize, page: PageResult,
                                                 presentations: [ReadingPresentation]) throws -> CGImage {
        let scale = size.width / Double(page.width)
        let verticalScale = size.height / Double(page.height)
        let content = ReadingPageOverlays(items: presentations, scale: scale, verticalScale: verticalScale)
            .frame(width: size.width, height: size.height, alignment: .topLeading).clipped()
        let renderer = ImageRenderer(content: content)
        renderer.scale = 1
        guard let result = renderer.cgImage, result.width == Int(size.width), result.height == Int(size.height) else {
            throw NativeCoreService.Failure(message: "无法按实际像素尺寸渲染这页漫画。")
        }
        return result
    }

    private static func composite(_ layer: CGImage, over image: CGImage) throws -> CGImage {
        // SwiftUI's working space can shift the colors of the source even outside the overlay.
        let sourceSpace = image.colorSpace
        let colorSpace = sourceSpace?.model == .rgb ? sourceSpace : CGColorSpace(name: CGColorSpace.sRGB)
        let bits = image.bitsPerComponent > 8 ? 16 : 8
        guard let colorSpace,
              let context = CGContext(data: nil, width: image.width, height: image.height, bitsPerComponent: bits,
                                      bytesPerRow: 0, space: colorSpace,
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
            throw CocoaError(.fileWriteUnknown)
        }
        let bounds = CGRect(x: 0, y: 0, width: image.width, height: image.height)
        context.interpolationQuality = .none
        context.setBlendMode(.copy)
        context.draw(image, in: bounds)
        context.setBlendMode(.normal)
        context.draw(layer, in: bounds)
        guard let result = context.makeImage() else { throw CocoaError(.fileWriteUnknown) }
        return result
    }
}

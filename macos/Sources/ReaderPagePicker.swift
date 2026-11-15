import SwiftUI
import ImageIO
import AppKit

private typealias ThumbnailState<Value> = SwiftUI.State<Value>

@MainActor private final class ReaderThumbnailCache {
    static let shared = ReaderThumbnailCache()
    private let images = NSCache<NSURL, NSImage>()

    private init() { images.countLimit = 160 }

    func image(for url: URL) async -> NSImage? {
        if let image = images.object(forKey: url as NSURL) { return image }
        let bitmap = await Task.detached(priority: .utility) {
            guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil as CGImage? }
            return CGImageSourceCreateThumbnailAtIndex(source, 0, [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceThumbnailMaxPixelSize: 160
            ] as CFDictionary)
        }.value
        guard !Task.isCancelled, let bitmap else { return nil }
        let image = NSImage(cgImage: bitmap, size: NSSize(width: bitmap.width, height: bitmap.height))
        images.setObject(image, forKey: url as NSURL)
        return image
    }
}

private struct ReaderThumbnail: View {
    let url: URL
    @ThumbnailState private var image: NSImage?

    var body: some View {
        Group {
            if let image { Image(nsImage: image).resizable().scaledToFit() }
            else { Image(systemName: "photo").font(.title2).foregroundStyle(ReaderPalette.secondary) }
        }.frame(width: 58, height: 80)
            .task(id: url) { image = await ReaderThumbnailCache.shared.image(for: url) }
            .accessibilityHidden(true)
    }
}

struct ReaderPagePicker: View {
    @ObservedObject var reader: Reader
    let select: (URL) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text(reader.folderName).font(.headline).lineLimit(1)
                Spacer()
                Text("\(reader.files.count) 页").font(.caption).foregroundStyle(ReaderPalette.secondary)
            }
            ScrollViewReader { proxy in
                ScrollView(.horizontal) {
                    LazyHStack(spacing: 8) {
                        ForEach(Array(reader.files.enumerated()), id: \.element) { index, url in
                            Button { select(url) } label: {
                                VStack(spacing: 6) {
                                    ReaderThumbnail(url: url)
                                    Text(url.lastPathComponent).font(.system(size: 11)).lineLimit(1).truncationMode(.middle)
                                    Text("\(index + 1)").font(.system(size: 11).monospacedDigit()).foregroundStyle(ReaderPalette.secondary)
                                }.frame(width: 88).padding(.vertical, 8)
                                    .background(reader.selected == url ? ReaderPalette.selection : .clear,
                                                in: RoundedRectangle(cornerRadius: 6))
                                    .contentShape(Rectangle())
                            }.buttonStyle(.plain).id(url).help(url.lastPathComponent)
                                .accessibilityIdentifier("page-thumbnail-\(index)")
                                .accessibilityLabel("第 \(index + 1) 页，" + url.lastPathComponent)
                        }
                    }.padding(.bottom, 6)
                }.onAppear { if let selected = reader.selected { proxy.scrollTo(selected, anchor: .center) } }
            }
        }.padding(16).frame(width: 560, height: 210)
            .foregroundStyle(ReaderPalette.text).background(ReaderPalette.bar)
    }
}

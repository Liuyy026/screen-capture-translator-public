import AppKit
import SwiftUI
import CryptoKit

// Build with -D KOMA_REVIEW_HARNESS into a separate app bundle. This uses the
// real Reader and ContentView, with copied page caches and isolated settings.
// No OCR/translation service starts, and no original translation is edited.
@main final class ReaderFlowReview: NSObject, NSApplicationDelegate {
    private var window: NSWindow!
    private var reader: Reader!
    private var output: URL!
    private var suite = ""

    static func main() {
        let application = NSApplication.shared
        let delegate = ReaderFlowReview()
        application.delegate = delegate
        application.setActivationPolicy(.regular)
        application.run()
        withExtendedLifetime(delegate) {}
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        Task { @MainActor in
            do { try await review() }
            catch {
                let failure = ["passed": false, "error": String(describing: error)] as [String: Any]
                if let output { try? JSONSerialization.data(withJSONObject: failure).write(to: output.appendingPathComponent("flow.json")) }
            }
            if !suite.isEmpty { UserDefaults.standard.removePersistentDomain(forName: suite) }
            reader?.shutdown()
            NSApplication.shared.terminate(nil)
        }
    }

    @MainActor private func settled() async throws {
        for _ in 0..<600 {
            if let error = reader.error ?? reader.readingLayoutError { throw failure(error) }
            if reader.readingRegions != nil { return }
            try await Task.sleep(for: .milliseconds(50))
        }
        throw failure("Timed out waiting for display assets")
    }

    private func failure(_ message: String) -> NSError {
        NSError(domain: "KomaReaderFlow", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
    }

    @MainActor private func capture(_ name: String) async throws {
        try await Task.sleep(for: .milliseconds(350))
        guard let view = window.contentView else { throw failure("No content view") }
        view.layoutSubtreeIfNeeded()
        guard let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { throw failure("Cannot capture owned view") }
        view.cacheDisplay(in: view.bounds, to: bitmap)
        guard let data = bitmap.representation(using: .png, properties: [:]) else { throw failure("Cannot encode snapshot") }
        try data.write(to: output.appendingPathComponent(name + ".png"))
    }

    @MainActor private func review() async throws {
        let args = CommandLine.arguments
        guard args.count == 4 else { throw failure("Expected repository, chapter, output paths") }
        let root = URL(fileURLWithPath: args[1]), folder = URL(fileURLWithPath: args[2])
        output = URL(fileURLWithPath: args[3])
        let manager = FileManager.default
        let copies = output.appendingPathComponent("page-copies")
        try manager.createDirectory(at: copies.appendingPathComponent("vision-v1"), withIntermediateDirectories: true)
        let originals = manager.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/KomaReader/pages/vision-v1")
        let images = try manager.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension.lowercased() == "webp" }.sorted { $0.lastPathComponent < $1.lastPathComponent }
        guard images.count >= 35 else { throw failure("Expected the 35-page review chapter") }
        for image in images {
            let key = SHA256.hash(data: try Data(contentsOf: image)).map { String(format: "%02x", $0) }.joined()
            try Data(contentsOf: originals.appendingPathComponent(key + ".json"))
                .write(to: copies.appendingPathComponent("vision-v1/" + key + ".json"))
        }
        suite = "local.koma.flow-review." + UUID().uuidString
        reader = Reader(root: root, cacheDir: copies, defaults: UserDefaults(suiteName: suite)!,
                        configurationStore: TranslationLocalConfigStore(url: output.appendingPathComponent("isolated-config.json")))
        reader.processingMode = "vision"
        reader.files = images
        reader.folderName = folder.lastPathComponent
        reader.displayMode = 2
        reader.zoom = 0.75
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1500, height: 1020),
                          styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.title = "Koma · 本地阅读流程检查"
        window.contentView = NSHostingView(rootView: ContentView(reader: reader, startsServices: false).tint(.teal))
        window.makeKeyAndOrderFront(nil)
        NSApplication.shared.activate(ignoringOtherApps: true)
        var checks: [String] = []

        reader.select(images[12]); try await settled()
        guard reader.page?.blocks.count == 17, reader.unreadableOverlayCount == 0 else { throw failure("Page 13 failed to load complete cached translations") }
        checks.append("existing_cache_loaded")
        try await capture("page-13")

        let cache = try reader.cacheFor(images[12])
        let saved = try Data(contentsOf: cache)
        let block = reader.page!.blocks[0]
        let revised = "本地检查：完整的新译文。"
        reader.saveEdit(id: block.id, source: block.source, translation: revised)
        try await settled()
        guard reader.readingPresentations.contains(where: { $0.text.contains(revised) }) else { throw failure("Edited translation did not replace the old presentation") }
        try await capture("edited-copy")
        try saved.write(to: cache)
        reader.reloadPage(); try await settled()
        guard !reader.readingPresentations.contains(where: { $0.text.contains(revised) }) else { throw failure("Restored cache retained stale edited text") }
        checks.append("edit_and_restore_refresh_display")

        reader.select(images[13]); try await Task.sleep(for: .milliseconds(20))
        reader.select(images[19]); try await Task.sleep(for: .milliseconds(20))
        reader.select(images[14]); try await settled()
        guard reader.selected == images[14], reader.page?.blocks.count == 10,
              reader.readingPresentations.flatMap({ $0.region.block_ids }).count == 10 else { throw failure("Rapid page changes applied a stale result") }
        checks.append("rapid_navigation_cancels_stale_results")
        try await capture("page-15")
        reader.move(-1); try await settled()
        guard reader.selected == images[13], reader.unreadableOverlayCount == 0 else { throw failure("Previous-page action failed") }
        try await capture("page-14")
        reader.displayMode = 0; try await capture("original-mode")
        reader.displayMode = 2
        reader.zoom = 0.5; try await capture("zoom-half")
        reader.zoom = 1; try await capture("zoom-full")
        checks.append("previous_page_modes_and_zoom")

        reader.select(images[33]); try await settled()
        guard reader.readingPreserved["5"] != nil, reader.unreadableOverlayCount == 0 else { throw failure("Footer review notice missing") }
        try await capture("page-34-footer")
        checks.append("footer_notice_separate_from_missing_translation")

        let badCache = try reader.cacheFor(images[33]), goodData = try Data(contentsOf: badCache)
        try Data("broken cache".utf8).write(to: badCache)
        reader.reloadPage()
        guard reader.page == nil, reader.readingRegions == nil, reader.error != nil else { throw failure("Invalid cache retained stale overlays") }
        try goodData.write(to: badCache)
        reader.select(images[33]); try await settled()
        checks.append("invalid_cache_fails_without_stale_overlay")
        let result: [String: Any] = ["passed": true, "checks": checks, "source_images_modified": false,
                                     "translation_services_started": false, "cache_edit_scope": "isolated copies"]
        try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
            .write(to: output.appendingPathComponent("flow.json"))
    }
}

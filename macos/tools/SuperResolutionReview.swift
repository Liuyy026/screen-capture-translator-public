import AppKit
import SwiftUI

/// Isolated real ContentView capture; no startup/model discovery/shared service calls.
@main final class SuperResolutionReview: NSObject, NSApplicationDelegate {
    private var window: NSWindow!
    private var reader: Reader!
    private var output: URL!
    static func main() {
        let app = NSApplication.shared, delegate = SuperResolutionReview()
        app.delegate = delegate; app.setActivationPolicy(.regular); app.run()
        withExtendedLifetime(delegate) {}
    }
    func applicationDidFinishLaunching(_ notification: Notification) {
        Task { @MainActor in
            do { try await review() }
            catch { try? JSONSerialization.data(withJSONObject: ["passed":false,"error":String(describing:error)]).write(to: output.appendingPathComponent("report.json")) }
            reader?.shutdown(); NSApp.terminate(nil)
        }
    }
    @MainActor func review() async throws {
        let args = CommandLine.arguments
        let root = URL(fileURLWithPath: args[1]), source = URL(fileURLWithPath: args[2])
        output = URL(fileURLWithPath: args[3])
        let suite = "local.koma.sr.window." + UUID().uuidString
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        reader = Reader(root: root, cacheDir: output.appendingPathComponent("pages"), defaults: defaults,
                        configurationStore: TranslationLocalConfigStore(url: output.appendingPathComponent("config.json")))
        reader.files = [source]; reader.displayMode = 2; reader.zoom = 0.6
        let engines = args.count > 4 ? args[4].split(separator: ",").map(String.init) : ["metalfx", "off"]
        let axis = args.count > 5 ? args[5] : "long_edge"
        guard let original = NSImage(contentsOf: source)?.representations.first else { throw NSError(domain:"Cannot read fixture",code:1) }
        let originalWidth = original.pixelsWide, originalHeight = original.pixelsHigh
        let edge = axis == "height" ? originalHeight : max(originalWidth, originalHeight)
        let enhancedHeight = edge >= 1440 ? originalHeight : Int((Double(originalHeight) * 1440 / Double(edge)).rounded())
        reader.srTarget = 1440; reader.srAxis = axis
        window = NSWindow(contentRect: NSRect(x: 0,y: 0,width: 1500,height: 1100),styleMask: [.titled,.closable,.resizable],backing: .buffered,defer: false)
        window.title = "Koma · 前置增强离线验证"
        window.appearance = NSAppearance(named: .aqua); window.backgroundColor = .white
        window.contentView = NSHostingView(rootView: ContentView(reader: reader, startsServices: false).tint(.teal).environment(\.colorScheme, .light).background(Color.white))
        window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
        var checks: [String] = []
        for engine in engines {
            reader.srEngine = engine
            reader.srPreset = engine == "realcugan" ? "conservative" : "standard"
            reader.changeEnhancement(); reader.select(source)
            let expectedHeight = engine == "off" ? originalHeight : enhancedHeight
            for _ in 0..<1000 {
                if let error = reader.error ?? reader.readingLayoutError { throw NSError(domain:error,code:1) }
                if reader.page?.height == expectedHeight && reader.readingRegions != nil && !reader.preparingImage { break }
                try await Task.sleep(for: .milliseconds(30))
            }
            guard reader.page?.height == expectedHeight, reader.readingRegions != nil, !reader.preparingImage else {
                throw NSError(domain:"Page did not settle: \(engine)",code:1)
            }
            try await capture(engine == "off" ? "original-chinese" : engine + "-chinese")
            checks.append("real ContentView: \(engine), height \(expectedHeight)")
        }
        try JSONSerialization.data(withJSONObject:["passed":true,"checks":checks,"shared_services_used":false,"translation":"fixed loopback test Chinese"]).write(to:output.appendingPathComponent("report.json"))
    }
    @MainActor func capture(_ name: String) async throws {
        try await Task.sleep(for: .milliseconds(500))
        let view = window.contentView!; view.layoutSubtreeIfNeeded()
        let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
        view.cacheDisplay(in: view.bounds,to: bitmap)
        try bitmap.representation(using: .png,properties: [:])!.write(to:output.appendingPathComponent(name + ".png"))
    }
}

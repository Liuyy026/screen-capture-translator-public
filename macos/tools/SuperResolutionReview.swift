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
        reader.srEngine = "metalfx"; reader.srTarget = 1440
        window = NSWindow(contentRect: NSRect(x: 0,y: 0,width: 1500,height: 1100),styleMask: [.titled,.closable,.resizable],backing: .buffered,defer: false)
        window.title = "Koma · 前置增强离线验证"
        window.appearance = NSAppearance(named: .aqua); window.backgroundColor = .white
        window.contentView = NSHostingView(rootView: ContentView(reader: reader, startsServices: false).tint(.teal).environment(\.colorScheme, .light).background(Color.white))
        window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
        reader.changeEnhancement(); reader.select(source)
        for _ in 0..<1000 {
            if reader.readingRegions != nil && !reader.preparingImage { break }
            if let error = reader.error ?? reader.readingLayoutError { throw NSError(domain:error,code:1) }
            try await Task.sleep(for: .milliseconds(30))
        }
        guard reader.page?.height == 1440, reader.readingRegions != nil else { throw NSError(domain:"Enhanced page did not settle",code:1) }
        try await capture("metalfx-chinese")
        reader.srEngine = "off"; reader.changeEnhancement()
        for _ in 0..<1000 {
            if reader.page?.height == 720 && reader.readingRegions != nil { break }
            try await Task.sleep(for: .milliseconds(30))
        }
        guard reader.page?.height == 720 else { throw NSError(domain:"Original page did not restore",code:1) }
        try await capture("original-chinese")
        try JSONSerialization.data(withJSONObject:["passed":true,"checks":["real ContentView enhanced Chinese display","disable restores original cached page"],"shared_services_used":false,"translation":"fixed loopback test Chinese"]).write(to:output.appendingPathComponent("report.json"))
    }
    @MainActor func capture(_ name: String) async throws {
        try await Task.sleep(for: .milliseconds(500))
        let view = window.contentView!; view.layoutSubtreeIfNeeded()
        let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
        view.cacheDisplay(in: view.bounds,to: bitmap)
        try bitmap.representation(using: .png,properties: [:])!.write(to:output.appendingPathComponent(name + ".png"))
    }
}

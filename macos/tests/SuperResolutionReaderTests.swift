import AppKit
import SwiftUI

@main struct SuperResolutionReaderTests {
    struct Failure: Error { let message: String }
    @MainActor static func main() async {
        do { try await run() } catch { print(error); exit(1) }
    }
    @MainActor static func run() async throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1])
        let output = URL(fileURLWithPath: CommandLine.arguments[2])
        let first = URL(fileURLWithPath: CommandLine.arguments[3])
        let second = URL(fileURLWithPath: CommandLine.arguments[4])
        let suite = "local.koma.sr.test." + UUID().uuidString
        let defaults = UserDefaults(suiteName: suite)!
        let reader = Reader(root: root, cacheDir: output.appendingPathComponent("pages"), defaults: defaults,
                            configurationStore: TranslationLocalConfigStore(url: output.appendingPathComponent("config.json")))
        defer { reader.shutdown(); defaults.removePersistentDomain(forName: suite) }
        func require(_ value: Bool, _ reason: String) throws { if !value { throw Failure(message: reason) } }
        func settled() async throws {
            for _ in 0..<1500 {
                if !reader.preparingImage { try await Task.sleep(for: .milliseconds(50)); return }
                try await Task.sleep(for: .milliseconds(20))
            }
            throw Failure(message: "prepare timed out")
        }
        func height() -> Int { (reader.image?.representations.first?.pixelsHigh) ?? 0 }
        reader.files = [first, second]
        reader.srEngine = "metalfx"; reader.select(first)
        try require(reader.preparingImage, "prepare must expose cancellation state")
        reader.cancel()
        try require(!reader.preparingImage && reader.page == nil, "cancel preparation immediately")
        // Switch source and configuration before the previous helper can complete.
        reader.srTarget = 2160; reader.changeEnhancement(); reader.select(second)
        reader.srTarget = 1440; reader.changeEnhancement(); reader.select(second)
        try await settled()
        try require(reader.selected == second && reader.files == [first,second], "source identity/list changed")
        try require(height() == 1440 && reader.error == nil, "late image from wrong setting/page accepted")
        reader.srEngine = "off"; reader.changeEnhancement()
        try await Task.sleep(for: .milliseconds(400))
        try require(height() == 720 && reader.page == nil && reader.error == nil, "disable did not restore source behavior")
        reader.srEngine = "realcugan"; reader.changeEnhancement(); try await settled()
        try require(reader.error != nil && height() == 720, "unavailable backend silently succeeded")
        reader.srEngine = "off"; reader.error = nil; reader.changeEnhancement()
        try await Task.sleep(for: .milliseconds(200))
        try require(reader.error == nil && height() == 720, "recovery after unavailable backend")
        let report: [String: Any] = ["passed":true, "checks":["preparation cancellation state", "rapid source and target switch rejects old result", "source list remains original", "disable restores original pixels", "missing backend fails visibly", "off recovers"]]
        try JSONSerialization.data(withJSONObject: report, options: .prettyPrinted).write(to: output.appendingPathComponent("reader-report.json"))
        print("passed")
    }
}

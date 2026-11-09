import AppKit
import SwiftUI

/// Controller tests with no window. Real window/capture validation remains a
/// separate app-bundle test; this executable does not claim that UI gate passed.
@main struct NativeReaderStateTests {
    struct Failure: Error { let message: String }
    @MainActor static func main() async {
        do { try await run() }
        catch { print("Reader controller failed: \(error)"); exit(1) }
    }
    @MainActor static func run() async throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1])
        let output = URL(fileURLWithPath: CommandLine.arguments[2])
        let endpoint = CommandLine.arguments[3]
        let manifest = try JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent(".runtime/rust-migration/pages.json"))) as! [[String: Any]]
        let images = manifest.map { URL(fileURLWithPath: $0["image"] as! String) }
        let suite = "local.koma.rust.state-test." + UUID().uuidString
        let defaults = UserDefaults(suiteName: suite)!
        let reader = Reader(root: root, cacheDir: output.appendingPathComponent("page-copies"), defaults: defaults,
                            configurationStore: TranslationLocalConfigStore(url: output.appendingPathComponent("config.json")))
        defer { reader.shutdown(); defaults.removePersistentDomain(forName: suite) }
        var checks: [String] = []
        func require(_ condition: Bool, _ message: String) throws {
            if !condition { throw Failure(message: message) }
        }
        func wait(_ condition: () -> Bool) async throws {
            for _ in 0..<1800 {
                if condition() { return }
                try await Task.sleep(for: .milliseconds(20))
            }
            throw Failure(message: "controller wait timed out")
        }
        func settled() async throws {
            try await wait { !reader.busy && (reader.readingRegions != nil || reader.error != nil || reader.readingLayoutError != nil) }
            try require(reader.error == nil && reader.readingLayoutError == nil, "page/display failed")
        }
        let connection = TranslationConnection(provider: "omlx", baseURL: endpoint, apiKey: "fake-secret")
        let models = try await reader.discoverModels(connection, allowLocalStart: true)
        try require(models == ["same-model"], "native model discovery")
        try reader.applyConnection(connection, model: "same-model", models: models, rememberKey: true)
        reader.processingMode = "vision"; reader.files = images
        reader.select(images[12]); try await settled()
        try require(reader.page?.blocks.count == 17 && reader.unreadableOverlayCount == 0, "page 13 complete")
        checks.append("native_model_discovery_and_page_load")
        let cache = try reader.cacheFor(images[12])
        let saved = try Data(contentsOf: cache)
        let block = reader.page!.blocks[0]
        let revised = "本地检查：完整的新译文。"
        reader.saveEdit(id: block.id, source: block.source, translation: revised)
        try await settled()
        try require(reader.readingPresentations.contains { $0.text.contains(revised) }, "edit display refresh")
        try saved.write(to: cache); reader.reloadPage(); try await settled()
        try require(!reader.readingPresentations.contains { $0.text.contains(revised) }, "restored display refresh")
        checks.append("native_edit_and_cache_restore")

        reader.select(images[13]); try await Task.sleep(for: .milliseconds(5))
        reader.select(images[19]); try await Task.sleep(for: .milliseconds(5))
        reader.select(images[14]); try await settled()
        try require(reader.selected == images[14] && reader.page?.blocks.count == 10, "rapid navigation page identity")
        try require(reader.readingPresentations.flatMap { $0.region.block_ids }.count == 10, "rapid navigation stale regions")
        checks.append("rapid_page_requests_reject_stale_results")
        reader.run(operation: "ocr", batch: true); try await settled()
        try require(reader.queueDone == 35 && reader.queueTotal == 35, "35-page queue progress")
        try require(reader.message.contains("处理完成"), "completion message overwritten by async reload")
        checks.append("35_page_native_queue_and_completion_status")

        let badCache = try reader.cacheFor(images[14]), good = try Data(contentsOf: badCache)
        try Data("invalid cache".utf8).write(to: badCache); reader.reloadPage()
        try await wait { reader.error != nil }
        try require(reader.page == nil && reader.readingRegions == nil, "invalid cache retained stale page")
        try good.write(to: badCache); reader.select(images[14]); try await settled()
        checks.append("invalid_cache_recovery")

        reader.chooseProcessingMode("text"); try await settled()
        let original = reader.page!.blocks
        let first = original[0], second = original[1]
        reader.saveEdit(id: second.id, source: second.source, translation: "保留人工修改")
        try await settled()
        reader.retryBlock(first.id); try await settled()
        try require(reader.page?.blocks.first(where: {$0.id == first.id})?.translation == "测试中文", "single block translation")
        try require(reader.page?.blocks.first(where: {$0.id == second.id})?.translation == "保留人工修改", "retry erased manual translation")
        checks.append("single_block_retry_preserves_manual_edit")

        let slow = output.appendingPathComponent("slow")
        let entered = output.appendingPathComponent("post-started")
        try Data().write(to: slow)
        reader.retryBlock(first.id)
        try await wait { FileManager.default.fileExists(atPath: entered.path) }
        reader.cancel(); try await wait { !reader.busy }
        try require(reader.message.contains("已停止"), "cancel status")
        try FileManager.default.removeItem(at: slow)
        try await settled()
        reader.retryBlock(first.id); try await settled()
        try require(reader.page?.blocks.first(where: {$0.id == first.id})?.translation == "测试中文", "retry after cancelled HTTP")
        checks.append("http_cancellation_and_next_job_recovery")

        let fail = output.appendingPathComponent("fail")
        try Data().write(to: fail)
        reader.retryBlock(first.id); try await settled()
        try require(reader.page?.blocks.first(where: {$0.id == first.id})?.error.isEmpty == false, "HTTP failure not reported")
        try FileManager.default.removeItem(at: fail)
        reader.retryBlock(first.id); try await settled()
        try require(reader.page?.blocks.first(where: {$0.id == first.id})?.error == "", "HTTP failure not recovered")
        try require(reader.page?.blocks.first(where: {$0.id == second.id})?.translation == "保留人工修改", "failure recovery erased manual text")
        checks.append("http_failure_and_explicit_retry")
        let report: [String: Any] = ["passed":true,"checks":checks,"window_test":false,
                                     "shared_services_used":false,"scope":"real Reader state and Rust service, private copied pages, loopback HTTP"]
        try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted,.sortedKeys])
            .write(to: output.appendingPathComponent("report.json"))
        print("Reader controller passed \(checks.count) scenarios; window validation remains separate.")
    }
}

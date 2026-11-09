import Foundation

enum ReadingOverlayService {
    struct Reply: Decodable {
        var version: Int
        var regions: [ReadingRegion]
        var preserved: [String: String]?
    }
    private struct Result: Decodable { var reply: Reply }
    @MainActor static func load(service: NativeCoreService, image: URL, cache: URL) async throws -> Reply {
        let data = try await service.request("display", values: ["image": image.path, "cache": cache.path])
        let result = try JSONDecoder().decode(Result.self, from: data)
        guard result.reply.version == 3 else { throw CocoaError(.fileReadCorruptFile) }
        return result.reply
    }
}

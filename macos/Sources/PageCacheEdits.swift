import Foundation

enum PageCacheEdits {
    // Preserve fields unknown to the reader, including the translation service
    // identity, structure, dialogues, and overlay geometry.
    static func update(_ url: URL, id: String, values: [String: Any]) throws {
        let original = try Data(contentsOf: url)
        guard var page = try JSONSerialization.jsonObject(with: original) as? [String: Any],
              var blocks = page["blocks"] as? [[String: Any]],
              let index = blocks.firstIndex(where: { $0["id"] as? String == id }) else {
            throw NSError(domain: "Koma", code: 3, userInfo: [NSLocalizedDescriptionKey: "缓存中没有找到这条对白，请重新打开图片。"])
        }
        for (key, value) in values { blocks[index][key] = value }
        page["blocks"] = blocks
        let data = try JSONSerialization.data(withJSONObject: page, options: [.prettyPrinted, .sortedKeys])
        try data.write(to: url, options: .atomic)
    }
}

import Foundation

@main struct CacheEditsTests {
    static func main() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("cache.json")
        let original: [String: Any] = [
            "translation_config": ["provider": "omlx", "base_url": "http://127.0.0.1:8000/v1", "model": "same-id"],
            "structure": ["signature": "keep"], "dialogues": [["id": "d1", "block_ids": ["1"]]],
            "overlay_regions": [["id": "r1", "points": [[1, 2], [3, 4]]]], "future_field": ["preserve": true],
            "blocks": [["id": "1", "source": "日文", "translation": "机器译文", "edited": false,
                        "error": "", "future_block_field": ["keep": true]],
                       ["id": "2", "source": "其他原文", "translation": "其他译文", "edited": false, "error": ""]]]
        try JSONSerialization.data(withJSONObject: original).write(to: url)
        try PageCacheEdits.update(url, id: "1", values: ["translation": "手改译文", "edited": true, "error": ""])
        let edited = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
        for key in ["translation_config", "structure", "dialogues", "overlay_regions", "future_field"] {
            precondition(NSDictionary(dictionary: [key: original[key]!]).isEqual(to: [key: edited[key]!]), key)
        }
        let blocks = edited["blocks"] as! [[String: Any]]
        precondition(blocks[0]["translation"] as? String == "手改译文")
        precondition(blocks[0]["edited"] as? Bool == true)
        precondition(blocks[0]["future_block_field"] != nil)
        precondition(NSDictionary(dictionary: blocks[1]).isEqual(to: (original["blocks"] as! [[String: Any]])[1]))
        try PageCacheEdits.update(url, id: "1", values: ["translation": "", "edited": false, "error": ""])
        let retried = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
        precondition(NSDictionary(dictionary: retried["translation_config"] as! [String: Any])
            .isEqual(to: original["translation_config"] as! [String: Any]))
        let before = try Data(contentsOf: url)
        do { try PageCacheEdits.update(url, id: "missing", values: ["translation": "bad"]); fatalError("Missing ID accepted") }
        catch { let after = try Data(contentsOf: url); precondition(after == before) }
        try Data("invalid JSON".utf8).write(to: url)
        do { try PageCacheEdits.update(url, id: "1", values: ["translation": "bad"]); fatalError("Invalid JSON accepted") }
        catch { let after = try Data(contentsOf: url); precondition(after == Data("invalid JSON".utf8)) }
        print("Cache edit/retry metadata preservation and failure-atomic checks passed")
    }
}

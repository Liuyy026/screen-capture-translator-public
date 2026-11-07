import Foundation
import Darwin

private struct TestFailure: Error { let message: String }
private func check(_ value: Bool, _ message: String) throws {
    if !value { throw TestFailure(message: message) }
}

@main struct ConnectionConfigTests {
    static func main() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = TranslationLocalConfigStore(url: directory.appendingPathComponent("settings/config.json"))
        try check(try store.load() == nil, "Missing file must be distinguishable from invalid configuration")
        let suite = "KomaConnectionConfigTests." + UUID().uuidString
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set("omlx", forKey: "translationProvider")
        defaults.set("HTTP://LOCALHOST:8000/", forKey: "translationURL.omlx")
        defaults.set("local-model", forKey: "translationModel.omlx|http://localhost:8000/v1")
        defaults.set("legacy-model", forKey: "model")
        let migrated = TranslationLocalConfiguration.migrating(defaults: defaults)
        try check(migrated.selectedProvider == "omlx", "Legacy provider migrates")
        try check(migrated.savedConnection("omlx")?.baseURL == "http://localhost:8000/v1", "Legacy endpoint normalizes")
        try check(migrated.savedConnection("omlx")?.model == "local-model", "Endpoint-specific model migrates")
        try check(migrated.savedConnection("ollama")?.model == "legacy-model", "Legacy Ollama model migrates")
        try check(migrated.connections.allSatisfy { $0.apiKey.isEmpty }, "Migration never imports secrets")
        let authenticated = TranslationConnection(provider: "omlx", baseURL: "http://localhost:8000/v1", apiKey: "test-key-do-not-log")
        let remembered = try migrated.applying(authenticated, model: "local-model", rememberKey: true)
        try store.save(remembered)
        try check(try store.load() == remembered, "Saved configuration survives a new read")
        let originalAttributes = try FileManager.default.attributesOfItem(atPath: store.url.path)
        try check((originalAttributes[.posixPermissions] as? NSNumber)?.intValue == 0o600, "New config has mode 0600")
        let originalInode = originalAttributes[.systemFileNumber] as? NSNumber
        let next = remembered.updatingModel("second-model", for: authenticated)
        try store.save(next)
        let updatedAttributes = try FileManager.default.attributesOfItem(atPath: store.url.path)
        try check((updatedAttributes[.posixPermissions] as? NSNumber)?.intValue == 0o600, "Replacement config keeps mode 0600")
        try check((updatedAttributes[.systemFileNumber] as? NSNumber) != originalInode, "Save replaces the file atomically")
        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: store.url.path)
        _ = try store.load()
        let tightened = try FileManager.default.attributesOfItem(atPath: store.url.path)
        try check((tightened[.posixPermissions] as? NSNumber)?.intValue == 0o600, "Reading tightens existing permissions")
        let sessionOnly = try next.applying(authenticated, model: "second-model", rememberKey: false)
        try store.save(sessionOnly)
        let restarted = try store.load()!
        try check(restarted.savedConnection("omlx")?.apiKey.isEmpty == true, "Unchecking remember removes a previously saved key")
        let changedModel = restarted.updatingModel("third-model", for: authenticated)
        try store.save(changedModel)
        try check(try store.load()?.savedConnection("omlx")?.apiKey.isEmpty == true,
                  "Changing models never persists a session-only key")
        let beforeOversized = try Data(contentsOf: store.url)
        let oversized = try changedModel.applying(TranslationConnection(provider: "omlx", baseURL: authenticated.baseURL,
                                                                        apiKey: String(repeating: "x", count: 1_048_577)),
                                                 model: "third-model", rememberKey: true)
        do { try store.save(oversized); throw TestFailure(message: "Oversized config accepted") }
        catch is TestFailure { throw TestFailure(message: "Oversized config accepted") }
        catch {}
        try check(try Data(contentsOf: store.url) == beforeOversized, "Rejected updates preserve existing bytes")
        let encoder = JSONEncoder()
        var invalidConfigurations = [Data("{not-json-test-key-do-not-log".utf8)]
        var unsupported = changedModel; unsupported.version = 2
        invalidConfigurations.append(try encoder.encode(unsupported))
        var duplicate = changedModel; duplicate.connections.append(duplicate.connections[0])
        invalidConfigurations.append(try encoder.encode(duplicate))
        var wrongSelection = changedModel; wrongSelection.selectedProvider = "unknown"
        invalidConfigurations.append(try encoder.encode(wrongSelection))
        var badEndpoint = changedModel; badEndpoint.connections[0].baseURL = "https://user:secret@localhost"
        invalidConfigurations.append(try encoder.encode(badEndpoint))
        for invalid in invalidConfigurations {
            try invalid.write(to: store.url)
            do { _ = try store.load(); throw TestFailure(message: "Invalid configuration accepted") }
            catch is TestFailure { throw TestFailure(message: "Invalid configuration accepted") }
            catch {
                try check(error.localizedDescription.contains(store.url.path), "Error identifies the file to repair")
                try check(!error.localizedDescription.contains("test-key-do-not-log"), "Errors never include key bytes")
            }
            do { try store.save(remembered); throw TestFailure(message: "Invalid config silently overwritten") }
            catch is TestFailure { throw TestFailure(message: "Invalid config silently overwritten") }
            catch {}
            try check(try Data(contentsOf: store.url) == invalid, "Invalid file remains intact for repair")
        }
        let symlink = directory.appendingPathComponent("linked.json")
        try FileManager.default.createSymbolicLink(at: symlink, withDestinationURL: store.url)
        do { _ = try TranslationLocalConfigStore(url: symlink).load(); throw TestFailure(message: "Symlink accepted") }
        catch is TestFailure { throw TestFailure(message: "Symlink accepted") }
        catch {}
        let fifo = directory.appendingPathComponent("fifo")
        guard mkfifo(fifo.path, 0o600) == 0 else { throw TestFailure(message: "Cannot prepare FIFO test") }
        do { _ = try TranslationLocalConfigStore(url: fifo).load(); throw TestFailure(message: "FIFO accepted") }
        catch is TestFailure { throw TestFailure(message: "FIFO accepted") }
        catch {}
        let leftovers = try FileManager.default.contentsOfDirectory(atPath: store.url.deletingLastPathComponent().path)
        try check(leftovers == ["config.json"], "Atomic writes clean up temporary files")
        print("Connection config migration, persistence, session-only keys, permissions, atomic replacement and invalid-file preservation passed")
    }
}

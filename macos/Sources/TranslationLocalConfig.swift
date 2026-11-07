import Foundation
import Darwin

struct TranslationConnection: Equatable {
    var provider: String
    var baseURL: String
    var apiKey: String

    var name: String { provider == "omlx" ? "oMLX" : "Ollama" }
    var identity: String { "\(provider)|\(baseURL)" }
    static func defaultURL(_ provider: String) -> String {
        provider == "omlx" ? "http://127.0.0.1:8000/v1" : "http://127.0.0.1:11439"
    }

    func normalized() throws -> TranslationConnection {
        guard ["ollama", "omlx"].contains(provider),
              var parts = URLComponents(string: baseURL.trimmingCharacters(in: .whitespacesAndNewlines)),
              ["http", "https"].contains(parts.scheme?.lowercased() ?? ""),
              let host = parts.host, !host.isEmpty,
              parts.user == nil, parts.password == nil,
              parts.query == nil, parts.fragment == nil else {
            throw ConnectionError("请输入有效的 http:// 或 https:// 接口地址，不要在地址中填写密钥。")
        }
        parts.scheme = parts.scheme?.lowercased()
        parts.host = host.lowercased()
        if let port = parts.port {
            guard (1...65535).contains(port) else { throw ConnectionError("接口端口无效。") }
            if (parts.scheme == "http" && port == 80) || (parts.scheme == "https" && port == 443) { parts.port = nil }
        }
        var path = parts.path
        while path.hasSuffix("/") { path.removeLast() }
        if provider == "omlx" && !path.hasSuffix("/v1") { path += "/v1" }
        parts.path = path
        guard let url = parts.url else { throw ConnectionError("接口地址无效。") }
        return TranslationConnection(provider: provider, baseURL: url.absoluteString,
                                     apiKey: apiKey.trimmingCharacters(in: .whitespacesAndNewlines))
    }
}

struct ConnectionError: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}

struct StoredTranslationConnection: Codable, Equatable {
    var provider: String
    var baseURL: String
    var model: String
    var apiKey: String

    var connection: TranslationConnection {
        TranslationConnection(provider: provider, baseURL: baseURL, apiKey: apiKey)
    }
}

struct TranslationLocalConfiguration: Codable, Equatable {
    var version = 1
    var selectedProvider: String
    var connections: [StoredTranslationConnection]

    func savedConnection(_ provider: String) -> StoredTranslationConnection? {
        connections.first { $0.provider == provider }
    }

    func validated() throws -> TranslationLocalConfiguration {
        guard version == 1, ["ollama", "omlx"].contains(selectedProvider),
              !connections.isEmpty, connections.count <= 2,
              Set(connections.map(\.provider)).count == connections.count,
              connections.contains(where: { $0.provider == selectedProvider }) else {
            throw ConnectionError("配置版本、服务选择或服务列表无效。")
        }
        var result = self
        result.connections = try connections.map { saved in
            let normalized = try saved.connection.normalized()
            return StoredTranslationConnection(provider: normalized.provider, baseURL: normalized.baseURL,
                                               model: saved.model, apiKey: normalized.apiKey)
        }
        return result
    }

    func applying(_ connection: TranslationConnection, model: String, rememberKey: Bool) throws -> TranslationLocalConfiguration {
        let connection = try connection.normalized()
        var result = self
        result.selectedProvider = connection.provider
        result.connections.removeAll { $0.provider == connection.provider }
        result.connections.append(StoredTranslationConnection(provider: connection.provider, baseURL: connection.baseURL,
                                                              model: model, apiKey: rememberKey ? connection.apiKey : ""))
        return try result.validated()
    }

    func updatingModel(_ model: String, for connection: TranslationConnection) -> TranslationLocalConfiguration {
        var result = self
        if let index = result.connections.firstIndex(where: { $0.connection.identity == connection.identity }) {
            result.connections[index].model = model
        }
        return result
    }

    // Only nonsecret preferences migrate. Keychain access is deliberately absent.
    static func migrating(defaults: UserDefaults) -> TranslationLocalConfiguration {
        let selected = defaults.string(forKey: "translationProvider") ?? "ollama"
        let connections = ["ollama", "omlx"].map { provider in
            let address = defaults.string(forKey: "translationURL." + provider) ?? TranslationConnection.defaultURL(provider)
            let connection = (try? TranslationConnection(provider: provider, baseURL: address, apiKey: "").normalized())
                ?? TranslationConnection(provider: provider, baseURL: TranslationConnection.defaultURL(provider), apiKey: "")
            let fallbackModel = provider == "ollama" && connection.baseURL == TranslationConnection.defaultURL("ollama")
                ? (defaults.string(forKey: "model") ?? "qwen3:4b") : ""
            let model = defaults.string(forKey: "translationModel." + connection.identity) ?? fallbackModel
            return StoredTranslationConnection(provider: provider, baseURL: connection.baseURL, model: model, apiKey: "")
        }
        return TranslationLocalConfiguration(selectedProvider: ["ollama", "omlx"].contains(selected) ? selected : "ollama",
                                             connections: connections)
    }
}

struct TranslationLocalConfigStore {
    static let defaultURL = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/Application Support/KomaReader/config.json")
    let url: URL

    init(url: URL = Self.defaultURL) { self.url = url }

    private func failure(_ reason: String) -> ConnectionError {
        ConnectionError("\(reason)\n配置文件：\(url.path)\n请修正该文件，或将它移走保留备份，再重新打开应用；现有配置不会被自动覆盖。")
    }

    func load() throws -> TranslationLocalConfiguration? {
        // Refuse symlinks and nonregular files; tighten existing files before reading secrets.
        let descriptor = open(url.path, O_RDONLY | O_NOFOLLOW | O_NONBLOCK)
        guard descriptor >= 0 else {
            if errno == ENOENT { return nil }
            throw failure("无法读取本地连接配置，请检查文件及权限。")
        }
        let handle = FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
        defer { try? handle.close() }
        var info = stat()
        guard fstat(descriptor, &info) == 0, (info.st_mode & S_IFMT) == S_IFREG,
              info.st_uid == getuid(), info.st_size <= 1_048_576,
              fchmod(descriptor, S_IRUSR | S_IWUSR) == 0 else {
            throw failure("本地连接配置必须是当前用户拥有、可读写的普通文件（不超过 1 MB）。")
        }
        do {
            let data = try handle.readToEnd() ?? Data()
            return try JSONDecoder().decode(TranslationLocalConfiguration.self, from: data).validated()
        } catch {
            // Decoder diagnostics may include file contents, so never forward them.
            throw failure("本地连接配置无效，请检查 JSON 格式、version、selectedProvider 和 connections。")
        }
    }

    func save(_ configuration: TranslationLocalConfiguration) throws {
        // An invalid existing file must remain available for repair, even on an explicit settings save.
        _ = try load()
        let validated: TranslationLocalConfiguration
        do { validated = try configuration.validated() }
        catch { throw failure("待保存的连接配置无效。") }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        let data = try encoder.encode(validated)
        guard data.count <= 1_048_576 else { throw failure("本地连接配置不能超过 1 MB。") }
        let directory = url.deletingLastPathComponent()
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true,
                                                    attributes: [.posixPermissions: 0o700])
        } catch { throw failure("无法建立本地连接配置目录，请检查目录权限。") }

        // mkstemp creates the temporary file with mode 0600, before any key bytes are written.
        var template = Array(directory.appendingPathComponent(".config.XXXXXX").path.utf8CString)
        let descriptor = mkstemp(&template)
        guard descriptor >= 0 else { throw failure("无法建立本地连接配置，请检查目录权限。") }
        let temporaryPath = String(cString: template)
        let handle = FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
        defer {
            try? handle.close()
            unlink(temporaryPath)
        }
        do {
            guard fchmod(descriptor, S_IRUSR | S_IWUSR) == 0 else { throw failure("无法限制配置文件权限。") }
            try handle.write(contentsOf: data)
            guard fsync(descriptor) == 0 else { throw failure("无法完成配置文件写入。") }
            guard rename(temporaryPath, url.path) == 0 else { throw failure("无法替换本地连接配置。") }
        } catch { throw failure("无法保存本地连接配置，请检查目录权限或可用空间。") }
    }
}

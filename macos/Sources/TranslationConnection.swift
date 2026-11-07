import SwiftUI
import Foundation

private typealias FormState<Value> = SwiftUI.State<Value>

private final class NoServiceRedirect: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    func urlSession(_ session: URLSession, task: URLSessionTask,
                    willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest,
                    completionHandler: @escaping (URLRequest?) -> Void) {
        completionHandler(nil)
    }
}

enum TranslationModels {
    static func fetch(_ raw: TranslationConnection) async throws -> [String] {
        let connection = try raw.normalized()
        let suffix = connection.provider == "omlx" ? "/models" : "/api/tags"
        guard let url = URL(string: connection.baseURL + suffix) else {
            throw ConnectionError("接口地址无效。")
        }
        var request = URLRequest(url: url)
        request.timeoutInterval = 8
        if !connection.apiKey.isEmpty {
            request.setValue("Bearer " + connection.apiKey, forHTTPHeaderField: "Authorization")
        }
        let config = URLSessionConfiguration.ephemeral
        config.connectionProxyDictionary = [:]
        let session = URLSession(configuration: config, delegate: NoServiceRedirect(), delegateQueue: nil)
        defer { session.invalidateAndCancel() }
        let data: Data
        let response: URLResponse
        do { (data, response) = try await session.data(for: request) }
        catch { throw ConnectionError("无法连接服务，请检查接口地址和服务是否已启动。") }
        guard let http = response as? HTTPURLResponse else { throw ConnectionError("服务返回了无效响应。") }
        if http.statusCode == 401 || http.statusCode == 403 {
            throw ConnectionError("认证失败，请检查 API Key。")
        }
        guard http.statusCode == 200 else {
            throw ConnectionError("获取模型失败（HTTP \(http.statusCode)），请检查接口地址。")
        }
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw ConnectionError("服务返回的模型列表不是有效 JSON。")
        }
        let rows = object[connection.provider == "omlx" ? "data" : "models"] as? [[String: Any]]
        guard let rows else { throw ConnectionError("服务返回的模型列表格式不正确。") }
        let ids = rows.filter {
            connection.provider == "omlx" || ($0["remote_host"] == nil && $0["remote_model"] == nil)
        }.compactMap { $0[connection.provider == "omlx" ? "id" : "name"] as? String }
            .filter { !$0.isEmpty && $0.count < 1024 && (connection.provider == "omlx" || !$0.lowercased().contains("cloud")) }
        return Array(Set(ids)).sorted()
    }
}

struct TranslationSettingsView: View {
    @ObservedObject var reader: Reader
    @FormState private var provider: String
    @FormState private var address: String
    @FormState private var apiKey: String
    @FormState private var keyAddress: String
    @FormState private var models: [String] = []
    @FormState private var selectedModel: String
    @FormState private var testing = false
    @FormState private var error: String?
    @FormState private var testedConnection: TranslationConnection?
    @FormState private var rememberKey = true
    @FormState private var connectionTask: Task<Void, Never>?

    init(reader: Reader) {
        self.reader = reader
        _provider = FormState(initialValue: reader.connection.provider)
        _address = FormState(initialValue: reader.connection.baseURL)
        _apiKey = FormState(initialValue: reader.connection.apiKey)
        _keyAddress = FormState(initialValue: reader.connection.baseURL)
        _selectedModel = FormState(initialValue: reader.model)
        _rememberKey = FormState(initialValue: reader.hasRememberedKey(reader.connection))
    }

    private var draft: TranslationConnection {
        TranslationConnection(provider: provider, baseURL: address, apiKey: apiKey)
    }
    private var canSave: Bool {
        !testing && testedConnection == (try? draft.normalized()) && models.contains(selectedModel)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("翻译接口设置").font(.title2.bold())
            Picker("翻译服务", selection: $provider) {
                Text("Ollama").tag("ollama")
                Text("oMLX").tag("omlx")
            }.pickerStyle(.segmented)
            TextField("接口地址", text: $address).textFieldStyle(.roundedBorder)
                .accessibilityLabel("接口地址")
            SecureField("API Key（按需填写）", text: $apiKey).textFieldStyle(.roundedBorder)
                .accessibilityLabel("API Key")
            Toggle("将 API Key 保存在本地配置文件", isOn: $rememberKey)
            Text("开发期间使用本地明文配置，仅当前用户可读写；不勾选时密钥仅在本次运行中使用。")
                .font(.caption).foregroundStyle(.secondary)
            Text("~/Library/Application Support/KomaReader/config.json")
                .font(.caption2).foregroundStyle(.secondary).textSelection(.enabled)
            if let configurationError = reader.configurationLoadError {
                Text(configurationError).font(.callout).foregroundStyle(.orange).textSelection(.enabled)
            }
            Text(provider == "omlx" ? "连接已启动的 oMLX；模型的加载与释放由 oMLX 管理。" :
                 "使用本机或其他设备上的 Ollama。默认本机地址可由应用自动启动。")
                .font(.caption).foregroundStyle(.secondary)
            HStack {
                Button(testing ? "正在连接…" : "连接并获取模型") { testConnection() }
                    .accessibilityIdentifier("translation-connect")
                    .disabled(testing)
                if testing { ProgressView().controlSize(.small) }
                if testedConnection == (try? draft.normalized()), !testing {
                    Text("已连接 · \(models.count) 个模型").font(.caption).foregroundStyle(.secondary)
                }
            }
            if !models.isEmpty {
                Picker("翻译模型", selection: $selectedModel) {
                    ForEach(models, id: \.self) { Text($0).tag($0) }
                }
            }
            if let error { Text(error).foregroundStyle(.orange).font(.callout).textSelection(.enabled) }
            HStack {
                Spacer()
                Button("取消") { reader.showConnectionSettings = false }
                    .accessibilityIdentifier("translation-cancel").keyboardShortcut(.cancelAction)
                Button("保存并使用") {
                    do {
                        try reader.applyConnection(try draft.normalized(), model: selectedModel,
                                                   models: models, rememberKey: rememberKey)
                        reader.showConnectionSettings = false
                    } catch { self.error = error.localizedDescription }
                }.accessibilityIdentifier("translation-save").keyboardShortcut(.defaultAction).disabled(!canSave)
            }
        }.padding(24).frame(width: 530)
            .onChange(of: provider) { _, value in
                cancelConnectionTest()
                let saved = reader.savedConnection(value)
                keyAddress = saved.baseURL
                address = saved.baseURL
                apiKey = saved.apiKey
                rememberKey = reader.hasRememberedKey(saved)
                selectedModel = reader.savedModel(provider: value, baseURL: address)
                models = []; testedConnection = nil; error = nil
            }
            .onChange(of: address) { old, value in
                if old != value {
                    cancelConnectionTest()
                    if keyAddress != value { apiKey = ""; keyAddress = value }
                    models = []; testedConnection = nil
                }
            }
            .onChange(of: apiKey) { _, _ in
                cancelConnectionTest()
                models = []; testedConnection = nil
            }
            .onDisappear { cancelConnectionTest() }
    }

    private func cancelConnectionTest() {
        connectionTask?.cancel()
        connectionTask = nil
        testing = false
        reader.releaseUnusedLocalOllama()
    }

    private func testConnection() {
        connectionTask?.cancel()
        testing = true; error = nil; testedConnection = nil
        let snapshot = draft
        connectionTask = Task { @MainActor in
            defer { if !Task.isCancelled { testing = false } }
            do {
                let normalized = try snapshot.normalized()
                let ids = try await reader.discoverModels(normalized, allowLocalStart: true)
                guard !Task.isCancelled, snapshot == draft else { return }
                models = ids
                if !ids.contains(selectedModel) { selectedModel = ids.first ?? "" }
                testedConnection = normalized
                if ids.isEmpty { error = "连接成功，但服务没有返回可用模型。" }
            } catch {
                guard !Task.isCancelled, snapshot == draft else { return }
                models = []; self.error = error.localizedDescription
            }
        }
    }
}

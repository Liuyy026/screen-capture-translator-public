import SwiftUI
import AppKit
import CryptoKit

// Keep property-wrapper state on SDKs that also export a State macro.
// Command Line Tools do not ship the SwiftUI macro plugin.
private typealias EditorState<Value> = SwiftUI.State<Value>

private struct NativeLoadedPage: Decodable { var cache: String; var page: PageResult?; var revision: String?; var effective_image: String? }

@MainActor final class Reader: ObservableObject {
    @Published var files: [URL] = []
    @Published var selected: URL?
    @Published var page: PageResult? { didSet { presentationCache = nil } }
    @Published var readingRegions: [ReadingRegion]? { didSet { presentationCache = nil } }
    @Published var readingPreserved: [String: String] = [:]
    private var presentationCache: [ReadingPresentation]?
    @Published var readingLayoutError: String?
    private var readingTask: Task<Void, Never>?
    private var readingRequest = UUID()
    private var readingCache: [String: ReadingOverlayService.Reply] = [:]
    @Published var image: NSImage?
    @Published var folderName = "本地漫画"
    @Published var busy = false
    @Published var preparingImage = false
    @Published var message = "打开漫画文件夹，开始阅读"
    @Published var error: String?
    @Published var selectedBlock: String?
    @Published var displayMode = 1
    @Published var zoom = 1.0
    @Published var model = "qwen3:4b"
    @Published var processingMode = "text"
    @Published var srEngine = "off"
    @Published var srTarget = 1440
    @Published var srAxis = "long_edge"
    @Published var srPreset = "standard"
    @Published var srStatus = ""
    private var effectiveImage: URL?
    var srSettings: [String: Any] { ["engine": srEngine, "target": srTarget, "axis": srAxis, "preset": srPreset] }
    func changeEnhancement() {
        if srEngine == "metalfx" { srPreset = "standard" }
        operationRequest = UUID()
        cancel(); pageTask?.cancel(); readingTask?.cancel()
        page = nil; cacheURL = nil; pageRevision = nil; effectiveImage = nil
        image = selected.flatMap { NSImage(contentsOf: $0) }
        reloadPage(updateMessage: true)
        Task {
            struct Backend: Decodable { var engine: String; var available: Bool; var presets: [String]; var reason: String }
            struct Capabilities: Decodable { var backends: [Backend] }
            do {
                let data = try await nativeCore.request("sr_capabilities")
                let backends = try JSONDecoder().decode(Capabilities.self, from: data).backends
                srStatus = backends.map { $0.engine + ($0.available ? "：可用（" + $0.presets.joined(separator: "、") + "）" : "：尚未准备 · " + $0.reason) }.joined(separator: "\n")
            } catch { srStatus = error.localizedDescription }
        }
    }
    @Published var availableModels: [String] = ["qwen3:4b"]
    @Published var connection = TranslationConnection(provider: "ollama", baseURL: TranslationConnection.defaultURL("ollama"), apiKey: "")
    @Published var showConnectionSettings = false
    @Published var connectionError: String?
    @Published var engineReady = false
    @Published var queueTotal = 0
    @Published var queueDone = 0
    private var task: Task<Void, Never>?
    private var pageTask: Task<Void, Never>?
    private var pageRequest = UUID()
    private var operationRequest = UUID()
    private var pageRevision: String?
    private var modelTimer: Timer?
    private var cacheURL: URL?
    private var started = false
    private var folderRequest = UUID()
    private var connectionRequest = UUID()
    private var sessionKeys: [String: String] = [:]
    private let connectionConfigStore: TranslationLocalConfigStore
    private let defaults: UserDefaults
    private var localConfiguration: TranslationLocalConfiguration
    private(set) var configurationLoadError: String?
    private var localOllamaProbes: Set<UUID> = []
    let root: URL
    var platformRoot: URL { root.appendingPathComponent("macos") }
    let cacheDir: URL
    private lazy var nativeCore: NativeCoreService = {
        let seed = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/KomaReader/pages")
        return NativeCoreService(root: root, cache: cacheDir,
                                 assets: cacheDir.deletingLastPathComponent().appendingPathComponent("reading-native-v1"),
                                 seed: FileManager.default.fileExists(atPath: seed.path) ? seed : nil)
    }()

    // The candidate bundle identifier already isolates .standard. Foundation
    // rejects using the application's own identifier as an explicit suite.
    init(root: URL? = nil, cacheDir: URL? = nil,
         defaults: UserDefaults = .standard,
         configurationStore: TranslationLocalConfigStore? = nil) {
        let configured = Bundle.main.object(forInfoDictionaryKey: "ProjectRoot") as? String
        self.root = root ?? URL(fileURLWithPath: configured ?? FileManager.default.currentDirectoryPath)
        self.cacheDir = cacheDir ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/" + ((Bundle.main.object(forInfoDictionaryKey: "CacheNamespace") as? String) ?? "KomaReaderRust") + "/pages")
        self.defaults = defaults
        self.connectionConfigStore = configurationStore ?? TranslationLocalConfigStore(url: self.cacheDir.deletingLastPathComponent().appendingPathComponent("config.json"))
        if configurationStore == nil, !defaults.bool(forKey: "stableReaderSettingsImported") {
            // The candidate has its own preferences; only import reading
            // position/mode once, without updating the stable application's domain.
            if let stable = UserDefaults(suiteName: "local.koma.reader") {
                for key in ["folder", "lastPage", "processingMode"] where defaults.object(forKey: key) == nil {
                    if let value = stable.object(forKey: key) { defaults.set(value, forKey: key) }
                }
            }
            defaults.set(true, forKey: "stableReaderSettingsImported")
        }
        self.localConfiguration = TranslationLocalConfiguration.migrating(defaults: defaults)
        do {
            if let saved = try self.connectionConfigStore.load() {
                localConfiguration = saved
            } else if configurationStore == nil, FileManager.default.fileExists(atPath: TranslationLocalConfigStore.defaultURL.path) {
                // Read stable configuration without changing its permissions or contents.
                let data = try Data(contentsOf: TranslationLocalConfigStore.defaultURL)
                localConfiguration = try JSONDecoder().decode(TranslationLocalConfiguration.self, from: data).validated()
                try self.connectionConfigStore.save(localConfiguration)
            } else {
                // Preserve legacy endpoint/model choices, without consulting Keychain.
                try self.connectionConfigStore.save(localConfiguration)
            }
        } catch {
            configurationLoadError = error.localizedDescription
            connectionError = configurationLoadError
        }
        connection = savedConnection(localConfiguration.selectedProvider)
        let preferred = savedModel(provider: connection.provider, baseURL: connection.baseURL)
        model = preferred.isEmpty && connection.provider == "ollama" ? "qwen3:4b" : preferred
        availableModels = model.isEmpty ? [] : [model]
        if connection.provider == "omlx", defaults.string(forKey: "processingMode") == "vision" {
            processingMode = "vision"
        }
    }

    func savedConnection(_ provider: String) -> TranslationConnection {
        var config = localConfiguration.savedConnection(provider)?.connection
            ?? TranslationConnection(provider: provider, baseURL: TranslationConnection.defaultURL(provider), apiKey: "")
        if let cached = sessionKeys[config.identity] { config.apiKey = cached }
        return config
    }

    func hasRememberedKey(_ config: TranslationConnection) -> Bool {
        guard let saved = localConfiguration.savedConnection(config.provider),
              saved.connection.identity == config.identity else { return false }
        return !saved.apiKey.isEmpty
    }

    func savedModel(provider: String, baseURL: String) -> String {
        guard let saved = localConfiguration.savedConnection(provider), saved.baseURL == baseURL else { return "" }
        return saved.model
    }

    private func persistModel(_ value: String) throws {
        if let configurationLoadError { throw ConnectionError(configurationLoadError) }
        let updated = localConfiguration.updatingModel(value, for: connection)
        guard updated != localConfiguration else { return }
        try connectionConfigStore.save(updated)
        localConfiguration = updated
    }

    func chooseModel(_ value: String) {
        guard !busy, availableModels.contains(value) else { return }
        do {
            try persistModel(value)
            model = value
            defaults.set(value, forKey: "translationModel." + connection.identity)
        } catch { connectionError = error.localizedDescription }
    }

    func chooseProcessingMode(_ value: String) {
        guard !busy, ["text", "vision"].contains(value), value != "vision" || connection.provider == "omlx" else { return }
        processingMode = value
        defaults.set(value, forKey: "processingMode")
        if value == "vision", availableModels.contains("Qwen3.8-27B-Uncensored-MLX-4bit") {
            chooseModel("Qwen3.8-27B-Uncensored-MLX-4bit")
        }
        if let selected { select(selected) }
    }

    func applyConnection(_ config: TranslationConnection, model: String, models: [String], rememberKey: Bool) throws {
        guard !busy else { throw ConnectionError("请等待当前任务结束后再切换接口。") }
        guard models.contains(model) else { throw ConnectionError("请先获取并选择一个模型。") }
        if let configurationLoadError { throw ConnectionError(configurationLoadError) }
        let config = try config.normalized()
        let updated = try localConfiguration.applying(config, model: model, rememberKey: rememberKey)
        try connectionConfigStore.save(updated)
        localConfiguration = updated
        sessionKeys[config.identity] = config.apiKey
        connectionRequest = UUID()
        connection = config
        availableModels = models
        self.model = model
        connectionError = nil
        engineReady = true
        defaults.set(config.provider, forKey: "translationProvider")
        defaults.set(config.baseURL, forKey: "translationURL." + config.provider)
        defaults.set(model, forKey: "translationModel." + config.identity)
        if config.provider != "omlx", processingMode == "vision" { chooseProcessingMode("text") }
        message = "已连接 " + config.name + " · " + model
        if config.provider != "ollama" || config.baseURL != TranslationConnection.defaultURL("ollama") {
            Task { _ = try? await nativeCore.request("release_engine") }
        }
    }

    func startup() {
        guard !started else { return }
        started = true
        startEngine()
        modelTimer = Timer.scheduledTimer(withTimeInterval: 10, repeats: true) { [weak self] _ in
            Task { @MainActor in _ = await self?.refreshModels() }
        }
        if let folder = defaults.string(forKey: "folder") {
            loadFolder(URL(fileURLWithPath: folder), restore: true)
        }
    }

    func discoverModels(_ config: TranslationConnection, allowLocalStart: Bool,
                        requireCurrentConnection: Bool = false) async throws -> [String] {
        let probe = UUID()
        let mayStartLocal = allowLocalStart && config.provider == "ollama" && config.apiKey.isEmpty
            && config.baseURL == TranslationConnection.defaultURL("ollama")
        if mayStartLocal { localOllamaProbes.insert(probe) }
        var completed = false
        defer {
            localOllamaProbes.remove(probe)
            if mayStartLocal && (!completed || Task.isCancelled) { releaseUnusedLocalOllama() }
        }
        guard !requireCurrentConnection || config == connection else { throw CancellationError() }
        let ids = try await nativeModels(config, allowLocalStart: allowLocalStart)
        try Task.checkCancellation()
        guard !requireCurrentConnection || config == connection else { throw CancellationError() }
        completed = true
        return ids
    }

    func releaseUnusedLocalOllama() {
        guard localOllamaProbes.isEmpty,
              connection.provider != "ollama" || connection.baseURL != TranslationConnection.defaultURL("ollama") else { return }
        Task { _ = try? await nativeCore.request("release_engine") }
    }

    private func nativeModels(_ connection: TranslationConnection, allowLocalStart: Bool = false) async throws -> [String] {
        struct Models: Decodable { var models: [String] }
        let data = try await nativeCore.request("models", values: ["provider": connection.provider,
            "base_url": connection.baseURL, "key": connection.apiKey, "allow_local_start": allowLocalStart])
        return try JSONDecoder().decode(Models.self, from: data).models
    }

    func startEngine() {
        Task { _ = await refreshModels(allowLocalStart: true) }
    }

    func refreshModels(allowLocalStart: Bool = false) async -> Bool {
        guard configurationLoadError == nil else {
            connectionError = configurationLoadError
            engineReady = false
            return false
        }
        guard !busy else { return engineReady }
        let snapshot = connection
        let request = UUID()
        connectionRequest = request
        do {
            let models = try await discoverModels(snapshot, allowLocalStart: allowLocalStart, requireCurrentConnection: true)
            guard request == connectionRequest, snapshot == connection, !busy else { return false }
            let selectedModel = models.contains(model) ? model : (models.first ?? "")
            if !selectedModel.isEmpty { try persistModel(selectedModel) }
            availableModels = models
            model = selectedModel
            if !model.isEmpty { defaults.set(model, forKey: "translationModel." + connection.identity) }
            engineReady = !models.isEmpty
            connectionError = models.isEmpty ? "连接成功，但服务没有返回可用模型。" : nil
            return true
        } catch {
            guard request == connectionRequest, snapshot == connection, !busy else { return false }
            engineReady = false
            connectionError = error.localizedDescription
            return false
        }
    }

    func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.prompt = "打开漫画"
        if panel.runModal() == .OK, let url = panel.url { loadFolder(url) }
    }

    func loadFolder(_ url: URL, restore: Bool = false) {
        guard !busy else { return }
        let request = UUID()
        folderRequest = request
        message = "正在读取漫画文件夹…"
        Task {
        do {
            let images = try await Task.detached(priority: .userInitiated) {
                let extensions: Set<String> = ["webp", "png", "jpg", "jpeg", "bmp", "tiff"]
                let contents = try FileManager.default.contentsOfDirectory(at: url, includingPropertiesForKeys: [.isRegularFileKey], options: [.skipsHiddenFiles])
                return contents.filter { extensions.contains($0.pathExtension.lowercased()) &&
                    ((try? $0.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) == true) }
                    .sorted { $0.lastPathComponent.localizedStandardCompare($1.lastPathComponent) == .orderedAscending }
            }.value
            guard request == folderRequest else { return }
            guard !images.isEmpty else { self.error = "这个文件夹里没有 WebP、PNG 或 JPG 图片。请选择直接包含漫画图片的文件夹。"; return }
            files = images; folderName = url.lastPathComponent
            defaults.set(url.path, forKey: "folder")
            let last = defaults.string(forKey: "lastPage")
            let initial = restore ? (images.first { $0.path == last } ?? images[0]) : images[0]
            select(initial)
        } catch { self.error = "无法打开文件夹：\(error.localizedDescription)" }
        }
    }

    func cacheFor(_ url: URL, mode: String? = nil) throws -> URL {
        let data = try Data(contentsOf: url, options: .mappedIfSafe)
        let digest = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        let directory = (mode ?? processingMode) == "vision" ? cacheDir.appendingPathComponent("vision-v1") : cacheDir
        return directory.appendingPathComponent(digest + ".json")
    }

    func select(_ url: URL) {
        effectiveImage = nil
        selected = url; selectedBlock = nil; error = nil
        image = NSImage(contentsOf: url)
        if image == nil { error = "无法解码这张图片：\(url.lastPathComponent)" }
        page = nil; cacheURL = nil; pageRevision = nil
        reloadPage(updateMessage: true)
        defaults.set(url.path, forKey: "lastPage")
    }

    func reloadPage(updateMessage: Bool = false) {
        pageTask?.cancel(); readingTask?.cancel()
        pageRequest = UUID(); readingRequest = UUID()
        readingRegions = nil; readingPreserved = [:]; readingLayoutError = nil
        guard let selected else { page = nil; return }
        let request = pageRequest, mode = processingMode
        let enhancement = srSettings
        preparingImage = srEngine != "off"
        pageTask = Task { [weak self] in
            guard let self else { return }
            defer { if request == pageRequest { preparingImage = false } }
            do {
                let data = try await nativeCore.request("load_page", values: ["image": selected.path, "mode": mode, "super_resolution": enhancement])
                let result = try JSONDecoder().decode(NativeLoadedPage.self, from: data)
                guard !Task.isCancelled, request == pageRequest, self.selected == selected, processingMode == mode else { return }
                effectiveImage = URL(fileURLWithPath: result.effective_image ?? selected.path)
                image = effectiveImage.flatMap { NSImage(contentsOf: $0) }
                cacheURL = URL(fileURLWithPath: result.cache); pageRevision = result.revision
                page = result.page
                if page != nil { prepareReadingRegions(cache: cacheURL!) }
                if updateMessage, !busy { message = page == nil ? "这页尚未处理 · 点击「识别并翻译」" : "已读取本地缓存 · \(page?.blocks.count ?? 0) 处文字" }
            } catch {
                guard !Task.isCancelled, request == pageRequest else { return }
                page = nil; self.error = "缓存无法读取：\(error.localizedDescription)"
            }
        }
    }

    var readingPresentations: [ReadingPresentation] {
        if let presentationCache { return presentationCache }
        guard let page, let readingRegions else { return [] }
        let items = ReadingLayout.presentations(page: page, regions: readingRegions)
        presentationCache = items
        return items
    }

    var unreadableOverlayCount: Int {
        guard let page else { return 0 }
        let shown = Set(readingPresentations.flatMap { $0.region.block_ids })
        return page.blocks.filter { $0.needsReadingOverlay && !shown.contains($0.id) && readingPreserved[$0.id] == nil }.count
    }

    private func prepareReadingRegions(cache: URL) {
        guard let selected = effectiveImage ?? selected, let data = try? Data(contentsOf: cache) else { return }
        let key = selected.path + ":" + SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        if let saved = readingCache[key] { readingRegions = saved.regions; readingPreserved = saved.preserved ?? [:]; return }
        let request = readingRequest
        readingTask = Task { [weak self] in
            guard let self else { return }
            do {
                let regions = try await ReadingOverlayService.load(service: nativeCore, image: selected, cache: cache)
                guard !Task.isCancelled, request == readingRequest else { return }
                if readingCache.count >= 16 { readingCache.removeAll() }
                readingCache[key] = regions
                readingRegions = regions.regions
                readingPreserved = regions.preserved ?? [:]
            } catch {
                guard !Task.isCancelled, request == readingRequest else { return }
                readingLayoutError = "排版分析暂不可用，原图已保留；请在右侧阅读译文。"
            }
        }
    }

    func move(_ offset: Int) {
        guard let selected, let i = files.firstIndex(of: selected), files.indices.contains(i + offset) else { return }
        select(files[i + offset])
    }

    func saveEdit(id: String, source: String, translation: String) {
        guard !busy, let p = page, let block = p.blocks.first(where: { $0.id == id }), let cacheURL else { return }
        let sourceChanged = source != block.source
        let selected = self.selected
        let revision = pageRevision
        busy = true
        task = Task {
            defer { busy = false; task = nil }
            do {
                var values: [String: Any] = ["cache": cacheURL.path, "block_id": id, "values": [
                    "source": source, "translation": sourceChanged ? "" : translation,
                    "edited": !sourceChanged && !translation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                    "error": "", "vision_source": "", "quality_warnings": []] as [String: Any]]
                if let revision { values["revision"] = revision }
                _ = try await nativeCore.request("edit", values: values)
                if self.selected == selected { reloadPage() }
                message = sourceChanged ? "日文已修改，旧译文已清除；点击翻译生成新译文" : "修改已保存；手改的译文会保留"
            } catch is CancellationError { message = "已停止；请重新打开页面确认保存结果" }
            catch { self.error = "保存失败：\(error.localizedDescription)" }
        }
    }

    func retryBlock(_ id: String) {
        guard !busy, page?.blocks.contains(where: { $0.id == id }) == true else { return }
        run(operation: "translate", blockID: id)
    }

    func run(operation: String = "both", batch: Bool = false, blockID: String? = nil) {
        guard !busy, let selected else { return }
        let urls = batch ? files : [selected]
        busy = true; error = nil; queueTotal = urls.count; queueDone = 0
        let jobConnection = connection, jobModel = model, jobMode = processingMode
        let jobEnhancement = srSettings
        let request = UUID(); operationRequest = request
        defaults.set(jobModel, forKey: "translationModel." + jobConnection.identity)
        task = Task { [self] in
            defer {
                // No newer processing task can start while this one owns busy.
                busy = false; task = nil
                if request == operationRequest { reloadPage() }
            }
            do {
                var values: [String: Any] = ["images": urls.map(\.path), "operation": operation,
                    "super_resolution": jobEnhancement, "mode": jobMode, "model": jobModel, "provider": jobConnection.provider,
                    "base_url": jobConnection.baseURL, "key": jobConnection.apiKey]
                if let blockID { values["block_id"] = blockID; values["retry"] = true }
                let data = try await nativeCore.request("process", values: values) { [weak self] event in
                    guard let self, request == operationRequest, !Task.isCancelled else { return }
                    switch event["event"] as? String {
                    case "progress": if let text = event["message"] as? String { message = text }
                    case "page_started":
                        if let path = event["image"] as? String { message = "正在处理 \(URL(fileURLWithPath: path).lastPathComponent)…" }
                    case "page_finished":
                        queueDone = event["done"] as? Int ?? queueDone
                        if let result = event["page"] as? [String: Any] {
                            if let text = result["error"] as? String { self.error = text }
                            if result["image"] as? String == self.selected?.path { reloadPage() }
                        }
                    default: break
                    }
                }
                try Task.checkCancellation()
                guard request == operationRequest else { return }
                let result = try JSONSerialization.jsonObject(with: data) as? [String: Any]
                let pages = result?["pages"] as? [[String: Any]] ?? []
                let failures = pages.reduce(0) { total, page in
                    total + ((page["ok"] as? Bool == true) ? ((page["result"] as? [String: Any])?["issues"] as? Int ?? 0) : 1)
                }
                queueDone = pages.count
                message = failures > 0 ? "处理结束，有 \(failures) 处问题；请查看提示后重试" : "处理完成 · \(queueDone) 页 · 结果已保存到本机"
            } catch is CancellationError { if request == operationRequest { message = "已停止；已完成的结果已保存" } }
            catch { if request == operationRequest { self.error = error.localizedDescription; message = "处理未完成；已保存的结果保留，可重试" } }
        }
    }

    func cancel() {
        if busy { message = "已停止；已完成的结果已保存" }
        else if preparingImage { message = "已停止前置增强；原图保留" }
        operationRequest = UUID()
        task?.cancel(); pageTask?.cancel(); readingTask?.cancel()
        pageRequest = UUID(); readingRequest = UUID(); preparingImage = false
    }
    func shutdown() {
        pageTask?.cancel(); readingTask?.cancel(); cancel(); modelTimer?.invalidate()
        nativeCore.shutdown()
    }

}

struct ContentView: View {
    @ObservedObject var reader: Reader
    var startsServices = true
    @StateObject private var imagePan = ImagePanController()
    var body: some View {
        HSplitView {
            sidebar.frame(minWidth: 170, idealWidth: 190, maxWidth: 250)
            VStack(spacing: 0) {
                HStack {
                    Text(reader.selected?.lastPathComponent ?? "漫画阅读器").font(.headline).lineLimit(1)
                    Spacer()
                    Picker("显示", selection: $reader.displayMode) {
                        Text("原图").tag(0); Text("文字框").tag(1); Text("中文").tag(2)
                    }.pickerStyle(.segmented).frame(width: 210)
                    Button { reader.zoom = max(0.5, reader.zoom - 0.25) } label: { Image(systemName: "minus.magnifyingglass") }
                    Text("\(Int(reader.zoom * 100))% ").monospacedDigit().font(.caption).frame(width: 42)
                    Button { reader.zoom = min(3, reader.zoom + 0.25) } label: { Image(systemName: "plus.magnifyingglass") }
                }.padding(14).background(.bar)
                if let image = reader.image {
                    GeometryReader { geo in
                        let ratio = image.size.height / max(image.size.width, 1)
                        let w = max(300, geo.size.width - 48) * reader.zoom
                        PannableImageScrollView(controller: imagePan,
                                               documentSize: CGSize(width: w + 48, height: w * ratio + 48),
                                               pageID: reader.selected) {
                            ZStack(alignment: .topLeading) {
                                Image(nsImage: image).resizable().frame(width: w, height: w * ratio)
                                if reader.displayMode != 0, let page = reader.page {
                                    if reader.displayMode == 2 {
                                        ForEach(reader.readingPresentations) { item in
                                            ReadingOverlayView(presentation: item, scale: w / Double(max(1, page.width)),
                                                               selected: item.region.block_ids.contains(reader.selectedBlock ?? "")) {
                                                reader.selectedBlock = item.region.block_ids.first
                                            }
                                        }
                                    }
                                    ForEach(page.blocks) { block in
                                        if reader.displayMode == 1 || !block.error.isEmpty {
                                            region(block, page: page, width: w, height: w * ratio)
                                        }
                                    }
                                }
                            }.frame(width: w, height: w * ratio).padding(24)
                                .contentShape(Rectangle())
                                .help("按住鼠标左键拖动图片；单击文字框定位对白。")
                        }
                        .onChange(of: reader.zoom) { _, _ in imagePan.endDrag() }
                    }.background(Color(nsColor: .underPageBackgroundColor))
                } else { welcome.frame(maxWidth: .infinity, maxHeight: .infinity) }
                HStack {
                    Button { reader.move(-1) } label: { Label("上一页", systemImage: "chevron.left") }
                        .disabled(reader.selected == reader.files.first || reader.files.isEmpty)
                    Spacer()
                    if let url = reader.selected, let i = reader.files.firstIndex(of: url) {
                        Text("\(i + 1) / \(reader.files.count)").font(.caption).monospacedDigit()
                    }
                    Spacer()
                    Button { reader.move(1) } label: { Label("下一页", systemImage: "chevron.right") }
                        .disabled(reader.selected == reader.files.last || reader.files.isEmpty)
                }.padding(12).background(.bar)
            }.frame(minWidth: 460)
            inspector.frame(minWidth: 270, idealWidth: 310, maxWidth: 420)
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            HStack(spacing: 10) {
                if reader.busy || reader.preparingImage { ProgressView().controlSize(.small) }
                else { Image(systemName: "internaldrive").foregroundStyle(.secondary) }
                Text(reader.message).lineLimit(2).font(.caption)
                Spacer()
                if reader.busy && reader.queueTotal > 1 { Text("\(reader.queueDone)/\(reader.queueTotal)").font(.caption) }
                Circle().fill(reader.engineReady ? .green : .orange).frame(width: 7, height: 7)
                Text(reader.engineReady ? "本地模型就绪" : "模型未就绪").font(.caption).foregroundStyle(.secondary)
            }.padding(.horizontal, 16).padding(.vertical, 9).background(.bar)
        }
        .toolbar {
            ToolbarItemGroup(placement: .navigation) {
                Button { reader.chooseFolder() } label: { Label("打开文件夹", systemImage: "folder") }.disabled(reader.busy)
            }
            ToolbarItemGroup(placement: .primaryAction) {
                if reader.busy || reader.preparingImage {
                    Button("停止", systemImage: "stop.fill") { reader.cancel() }
                } else {
                    Menu {
                        Button("只识别本页日文") { reader.run(operation: "ocr") }
                        Button("翻译整个文件夹（可随时停止）") { reader.run(batch: true) }
                            .disabled(!reader.engineReady)
                    } label: { Image(systemName: "ellipsis.circle") }.disabled(reader.selected == nil)
                    Button { reader.run() } label: { Label("识别并翻译", systemImage: "character.bubble") }
                        .buttonStyle(.borderedProminent).tint(.teal)
                        .disabled(reader.selected == nil || !reader.engineReady)
                }
            }
        }
        .frame(minWidth: 1040, minHeight: 680)
        .sheet(isPresented: $reader.showConnectionSettings) { TranslationSettingsView(reader: reader) }
        .task { if startsServices { reader.startup() } }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.willTerminateNotification)) { _ in reader.shutdown() }
    }

    var sidebar: some View {
        VStack(alignment: .leading, spacing: 0) {
            Label("KOMA", systemImage: "book.closed.fill").font(.system(size: 19, weight: .bold, design: .rounded)).foregroundStyle(.teal).padding(18)
            Text(reader.folderName).font(.headline).lineLimit(2).padding(.horizontal, 18)
            Text("\(reader.files.count) 页 · 本地文件").font(.caption).foregroundStyle(.secondary).padding(.horizontal, 18).padding(.top, 4).padding(.bottom, 12)
            List(selection: Binding(get: { reader.selected }, set: { if let url = $0 { reader.select(url) } })) {
                ForEach(Array(reader.files.enumerated()), id: \.element) { i, url in
                    HStack(spacing: 10) {
                        Text(String(format: "%02d", i + 1)).font(.caption.monospacedDigit()).foregroundStyle(.secondary).frame(width: 28)
                        Text(url.lastPathComponent).lineLimit(1).font(.system(size: 12))
                    }.padding(.vertical, 7).tag(url)
                }
            }.listStyle(.sidebar)
            Divider()
            VStack(alignment: .leading, spacing: 7) {
                Label("完全本地处理", systemImage: "lock.shield").font(.caption.weight(.medium))
                Text("原图保持不变\n识别与译文自动缓存").font(.caption).foregroundStyle(.secondary).lineSpacing(4)
            }.padding(18)
        }
    }

    var welcome: some View {
        VStack(spacing: 20) {
            Image(systemName: "text.viewfinder").font(.system(size: 62, weight: .ultraLight)).foregroundStyle(.teal)
            Text("把日文，读成故事。").font(.system(size: 28, weight: .semibold, design: .rounded))
            Text("打开装有漫画的文件夹，自动寻找对白。\n支持 WebP、PNG、JPG，无需截图框选。")
                .multilineTextAlignment(.center).foregroundStyle(.secondary).lineSpacing(6)
            Button("打开漫画文件夹") { reader.chooseFolder() }.buttonStyle(.borderedProminent).controlSize(.large).tint(.teal)
            Button("先试试内置测试页") { reader.loadFolder(reader.root.appendingPathComponent("samples")) }.buttonStyle(.link)
        }.padding(30)
    }

    func region(_ block: TextBlock, page: PageResult, width: Double, height: Double) -> some View {
        let x = block.box[0] / Double(page.width) * width
        let y = block.box[1] / Double(page.height) * height
        let w = max(12, (block.box[2] - block.box[0]) / Double(page.width) * width)
        let h = max(12, (block.box[3] - block.box[1]) / Double(page.height) * height)
        let active = reader.selectedBlock == block.id
        return ZStack(alignment: .topLeading) {
            RoundedRectangle(cornerRadius: 3).fill(.teal.opacity(active ? 0.18 : 0.05))
            RoundedRectangle(cornerRadius: 3).strokeBorder(block.error.isEmpty ? Color.teal : Color.orange, lineWidth: active ? 3 : 1)
            if reader.displayMode == 1 || !block.error.isEmpty {
                Text(block.id).font(.system(size: 10, weight: .bold)).foregroundStyle(.white).padding(.horizontal, 4).padding(.vertical, 2).background(.teal, in: RoundedRectangle(cornerRadius: 3)).offset(y: -16)
            }
        }.frame(width: w, height: h)
            .contentShape(Rectangle()).onTapGesture { reader.selectedBlock = block.id }
            .help(block.translation.isEmpty ? block.source : block.translation)
            .offset(x: x, y: y)
    }

    var inspector: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("对白对照").font(.title3.weight(.semibold))
            if reader.displayMode == 2, reader.page != nil, reader.readingRegions == nil, reader.readingLayoutError == nil {
                Text("正在整理中文排版…").font(.caption).foregroundStyle(.secondary)
            }
            if reader.displayMode == 2, reader.readingRegions != nil, reader.unreadableOverlayCount > 0 {
                Text("有 \(reader.unreadableOverlayCount) 处已有译文暂未覆盖到图片，完整中文可在下方阅读。")
                    .font(.caption).foregroundStyle(.secondary)
            }
            if let layoutError = reader.readingLayoutError {
                Text(layoutError).font(.caption).foregroundStyle(.orange)
            }
            if reader.displayMode == 2, !reader.readingPreserved.isEmpty {
                Text("有 \(reader.readingPreserved.count) 处疑似页码，已保留原图，请核对右侧原文。")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Group {
                Picker("前置增强", selection: $reader.srEngine) {
                    Text("关闭（原图）").tag("off")
                    Text("MetalFX 空间超分").tag("metalfx")
                    Text("Real-CUGAN").tag("realcugan")
                }.onChange(of: reader.srEngine) { _, _ in reader.changeEnhancement() }
                if reader.srEngine != "off" {
                    Picker("目标尺寸", selection: $reader.srTarget) {
                        Text("1440").tag(1440); Text("2160").tag(2160)
                    }.onChange(of: reader.srTarget) { _, _ in reader.changeEnhancement() }
                    Picker("按", selection: $reader.srAxis) {
                        Text("长边").tag("long_edge"); Text("高度").tag("height")
                    }.onChange(of: reader.srAxis) { _, _ in reader.changeEnhancement() }
                    Picker("增强预设", selection: $reader.srPreset) {
                        Text("标准").tag("standard")
                        if reader.srEngine == "realcugan" {
                            Text("保守").tag("conservative"); Text("降噪").tag("denoise")
                        }
                    }.onChange(of: reader.srPreset) { _, _ in reader.changeEnhancement() }
                    Text("已达目标默认跳过。原图保留；增强后的识别与译文单独保存，不迁移原图手改内容。")
                        .font(.caption2).foregroundStyle(.secondary)
                }
                if !reader.srStatus.isEmpty { Text(reader.srStatus).font(.caption2).foregroundStyle(.secondary) }
            }.disabled(reader.busy)
            Picker("翻译方式", selection: Binding(get: { reader.processingMode }, set: { reader.chooseProcessingMode($0) })) {
                Text("普通翻译").tag("text")
                Text("视觉辅助").tag("vision")
            }.pickerStyle(.segmented).disabled(reader.busy || reader.connection.provider != "omlx")
                .accessibilityIdentifier("translation-mode")
            if reader.processingMode == "vision" {
                Text("结合整页画面与 OCR 翻译；需要支持图片的 oMLX 模型。结果单独保存，日文修正需手动核对。")
                    .font(.caption2).foregroundStyle(.secondary)
            }
            HStack {
                Text("翻译服务：" + reader.connection.name).font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("接口设置…") { reader.showConnectionSettings = true }.disabled(reader.busy)
            }
            Picker("翻译模型", selection: Binding(get: { reader.model }, set: { reader.chooseModel($0) })) {
                if reader.availableModels.isEmpty { Text("请连接服务并获取模型").tag("") }
                ForEach(reader.availableModels, id: \.self) { Text($0).tag($0) }
            }.disabled(reader.busy || !reader.engineReady)
            HStack {
                Text(reader.engineReady ? "已连接" : "未连接").font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("刷新模型") { reader.startEngine() }.font(.caption).disabled(reader.busy)
            }
            if let connectionError = reader.connectionError {
                Text(connectionError).font(.caption).foregroundStyle(.orange)
            }
            if let error = reader.error {
                Text(error).font(.caption).foregroundStyle(.orange).textSelection(.enabled)
            }
            Divider()
            if let page = reader.page {
                HStack {
                    Text("\(page.blocks.count) 处文字").font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    if !page.model.isEmpty { Text(page.model).font(.caption2).foregroundStyle(.secondary) }
                }
                if page.blocks.isEmpty {
                    Text("没有检测到文字。复杂背景、手写字或低清晰度图片可能漏检。").foregroundStyle(.secondary).font(.callout)
                }
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(spacing: 12) {
                            ForEach(page.blocks) { block in
                                BlockEditor(block: block, active: reader.selectedBlock == block.id,
                                            disabled: reader.busy, readingNote: reader.readingPreserved[block.id], save: { source, target in
                                    reader.saveEdit(id: block.id, source: source, translation: target)
                                }, retry: { reader.retryBlock(block.id) }, select: { reader.selectedBlock = block.id })
                                .id(block.id)
                            }
                        }
                    }.onChange(of: reader.selectedBlock) { _, id in
                        if let id { withAnimation { proxy.scrollTo(id, anchor: .top) } }
                    }
                }
            } else {
                VStack(alignment: .leading, spacing: 12) {
                    Image(systemName: "character.bubble").font(.largeTitle).foregroundStyle(.tertiary)
                    Text("识别后，日文和中文会出现在这里。").font(.callout).foregroundStyle(.secondary)
                    Text("点击图上的文字框可定位对白。可以修正日文或中文，然后保存。").font(.caption).foregroundStyle(.secondary)
                }.padding(.top, 30)
            }
            Spacer(minLength: 0)
            Text("自动顺序与译文可能有误，请结合原图检查。中文覆盖为试验功能。")
                .font(.caption2).foregroundStyle(.secondary)
        }.padding(16)
    }
}

struct BlockEditor: View {
    let block: TextBlock
    let active: Bool
    let disabled: Bool
    let readingNote: String?
    let save: (String, String) -> Void
    let retry: () -> Void
    let select: () -> Void
    @EditorState private var source = ""
    @EditorState private var target = ""
    var changed: Bool { source != block.source || target != block.translation }
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("\(block.id)").font(.caption.bold()).foregroundStyle(.teal)
                Text(block.vertical ? "原文竖排" : "原文横排").font(.caption2).foregroundStyle(.secondary)
                Spacer()
                if block.edited { Text("已手动修改").font(.caption2).foregroundStyle(.teal) }
            }
            TextField("日文原文", text: $source, axis: .vertical).textFieldStyle(.plain).font(.system(size: 13)).lineLimit(2...8)
            if let suggestion = block.vision_source, !suggestion.isEmpty, suggestion != block.source {
                Text("画面复核建议：" + suggestion).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                Button("填入日文，核对后保存") { source = suggestion }.font(.caption).disabled(disabled)
            }
            Divider()
            TextField("尚未翻译", text: $target, axis: .vertical).textFieldStyle(.plain).font(.system(size: 14)).lineLimit(2...10)
            if !block.error.isEmpty { Text(block.error).font(.caption).foregroundStyle(.orange).textSelection(.enabled) }
            ForEach((block.quality_warnings ?? []).filter { $0.hasPrefix("原文与译文相同") }, id: \.self) { warning in
                Text(warning).font(.caption2).foregroundStyle(.orange)
            }
            if let reason = readingNote {
                Text(reason).font(.caption).foregroundStyle(.secondary)
            }
            HStack {
                Button("重新翻译", action: retry).font(.caption).disabled(disabled || changed)
                Spacer()
                if changed { Button("保存修改") { save(source, target) }.font(.caption).disabled(disabled) }
            }
        }.padding(12).background(active ? Color.teal.opacity(0.08) : Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(active ? Color.teal : Color.gray.opacity(0.15)))
            .disabled(disabled).onTapGesture(perform: select)
            .onAppear { source = block.source; target = block.translation }
            .onChange(of: block.source) { _, value in source = value }
            .onChange(of: block.translation) { _, value in target = value }
    }
}

#if !KOMA_REVIEW_HARNESS
@main struct KomaReaderApp: App {
    @StateObject private var reader = Reader()
    var body: some Scene {
        WindowGroup("Koma Rust · 本地漫画翻译") { ContentView(reader: reader).tint(.teal) }
            .defaultSize(width: 1280, height: 850)
            .commands {
                CommandGroup(after: .appSettings) {
                    Button("翻译接口设置…") { reader.showConnectionSettings = true }
                        .keyboardShortcut(",").disabled(reader.busy)
                }
                CommandGroup(replacing: .newItem) {
                    Button("打开漫画文件夹…") { reader.chooseFolder() }.keyboardShortcut("o").disabled(reader.busy)
                }
            }
    }
}
#endif

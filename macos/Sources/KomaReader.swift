import SwiftUI
import AppKit
import CryptoKit

private struct NativeLoadedPage: Decodable {
    var cache: String
    var page: PageResult?
    var revision: String?
    var effective_image: String?
    var layout_image: String?
    var enhancement_warning: String?
}

@MainActor final class Reader: ObservableObject {
    static let enhancementTargets = [2560, 3840]

    @Published var files: [URL] = []
    @Published var selected: URL?
    @Published var page: PageResult? { didSet { presentationCache = nil } }
    @Published var readingRegions: [ReadingRegion]? { didSet { presentationCache = nil } }
    @Published var readingPreserved: [String: String] = [:]
    private var presentationCache: [ReadingPresentation]?
    @Published var readingLayoutError: String?
    @Published private(set) var loadingPage = false
    @Published private(set) var pageLoadError: String?
    @Published private(set) var enhancementWarning: String?
    @Published private(set) var pageProcessingErrors: [URL: String] = [:]
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
    @Published var srTarget = Reader.enhancementTargets[0]
    @Published var srAxis = "long_edge"
    @Published var srPreset = "standard"
    @Published var srStatus = ""
    private var effectiveImage: URL?
    var displaysEnhancedImage: Bool {
        guard let effectiveImage, let selected else { return false }
        return effectiveImage.path != selected.path
    }
    private var layoutImage: URL?
    var srSettings: [String: Any] { ["engine": srEngine, "target": srTarget, "axis": srAxis, "preset": srPreset] }
    func changeEnhancement() {
        if srEngine == "metalfx" { srPreset = "standard" }
        error = nil; srStatus = ""
        page = nil; cacheURL = nil; pageRevision = nil; effectiveImage = nil; layoutImage = nil
        image = selected.flatMap { NSImage(contentsOf: $0) }
        reloadPage(updateMessage: true)
        Task {
            struct Backend: Decodable { var engine: String; var available: Bool; var presets: [String]; var reason: String }
            struct Capabilities: Decodable { var backends: [Backend] }
            do {
                let data = try await nativeCore.request("sr_capabilities")
                let backends = try JSONDecoder().decode(Capabilities.self, from: data).backends
                let names = ["metalfx": "MetalFX", "realcugan": "Real-CUGAN"]
                let presets = ["standard": "标准", "conservative": "保守", "denoise": "降噪"]
                srStatus = backends.map { backend in
                    let name = names[backend.engine] ?? backend.engine
                    return name + (backend.available
                        ? "：可用（" + backend.presets.map { presets[$0] ?? $0 }.joined(separator: "、") + "）"
                        : "：尚未准备，请检查本地增强组件")
                }.joined(separator: "\n")
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
    @Published private(set) var exportingChapter = false
    @Published private(set) var lastChapterExport: URL?
    private var task: Task<Void, Never>?
    private var pageTask: Task<Void, Never>?
    private var pageRequest = UUID()
    private var operationRequest = UUID()
    private var pageRevision: String?
    private var modelTimer: Timer?
    private var cacheURL: URL?
    var editCacheIdentity: String? { cacheURL?.path }
    private var started = false
    @Published private var folderRequest: UUID?
    var openingFolder: Bool { folderRequest != nil }
    private var connectionRequest: UUID?
    private var sessionKeys: [String: String] = [:]
    private let connectionConfigStore: TranslationLocalConfigStore
    var connectionConfigURL: URL { connectionConfigStore.url }
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
            } else if configurationStore == nil, let stable = try TranslationLocalConfigStore().load(readOnly: true) {
                localConfiguration = stable
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
        connectionRequest = nil
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
        guard !busy, connectionRequest == nil else { return engineReady }
        let snapshot = connection
        let request = UUID()
        connectionRequest = request
        defer { if request == connectionRequest { connectionRequest = nil } }
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
            return engineReady
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
            defer { if request == folderRequest { folderRequest = nil } }
            do {
                let images = try await Task.detached(priority: .userInitiated) {
                    let extensions: Set<String> = ["webp", "png", "jpg", "jpeg", "bmp", "tif", "tiff"]
                    let contents = try FileManager.default.contentsOfDirectory(at: url, includingPropertiesForKeys: [.isRegularFileKey], options: [.skipsHiddenFiles])
                    return contents.filter { extensions.contains($0.pathExtension.lowercased()) &&
                        ((try? $0.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) == true) }
                        .sorted { $0.lastPathComponent.localizedStandardCompare($1.lastPathComponent) == .orderedAscending }
                }.value
                guard request == folderRequest else { return }
                guard !images.isEmpty else {
                    self.error = "这个文件夹里没有支持的漫画图片。请选择直接包含图片的文件夹。"
                    message = "未打开新文件夹"
                    return
                }
                pageProcessingErrors.removeAll()
                files = images; folderName = url.lastPathComponent
                defaults.set(url.path, forKey: "folder")
                let last = defaults.string(forKey: "lastPage")
                let initial = restore ? (images.first { $0.path == last } ?? images[0]) : images[0]
                select(initial)
            } catch {
                if request == folderRequest {
                    self.error = "无法打开文件夹：\(error.localizedDescription)"
                    message = "未打开新文件夹"
                }
            }
        }
    }

    func cacheFor(_ url: URL, mode: String? = nil) throws -> URL {
        let data = try Data(contentsOf: url, options: .mappedIfSafe)
        let digest = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        let directory = (mode ?? processingMode) == "vision" ? cacheDir.appendingPathComponent("vision-v1") : cacheDir
        return directory.appendingPathComponent(digest + ".json")
    }

    func select(_ url: URL) {
        effectiveImage = nil; layoutImage = nil
        selected = url; selectedBlock = nil; error = nil
        image = NSImage(contentsOf: url)
        if image == nil { error = "无法解码这张图片：\(url.lastPathComponent)" }
        page = nil; cacheURL = nil; pageRevision = nil
        reloadPage(updateMessage: !busy)
        defaults.set(url.path, forKey: "lastPage")
    }

    private func resetReadingRegions() {
        readingTask?.cancel()
        readingRequest = UUID()
        readingRegions = nil; readingPreserved = [:]; readingLayoutError = nil
    }

    func reloadPage(updateMessage: Bool = false, useOriginalImage: Bool = false) {
        pageTask?.cancel()
        pageRequest = UUID()
        resetReadingRegions()
        guard let selected else { page = nil; return }
        let request = pageRequest, mode = processingMode
        var enhancement = srSettings
        if useOriginalImage { enhancement["engine"] = "off" }
        loadingPage = true; pageLoadError = nil; enhancementWarning = nil
        preparingImage = !useOriginalImage && srEngine != "off"
        let loadingMessage = "正在读取本页结果…"
        if updateMessage, !busy { message = loadingMessage }
        pageTask = Task { [weak self] in
            guard let self else { return }
            defer {
                if request == pageRequest { loadingPage = false; preparingImage = false }
            }
            do {
                let data = try await nativeCore.request("load_page", values: ["image": selected.path, "mode": mode, "super_resolution": enhancement])
                let result = try JSONDecoder().decode(NativeLoadedPage.self, from: data)
                guard !Task.isCancelled, request == pageRequest, self.selected == selected, processingMode == mode else { return }
                effectiveImage = URL(fileURLWithPath: result.effective_image ?? selected.path)
                layoutImage = URL(fileURLWithPath: result.layout_image ?? result.effective_image ?? selected.path)
                image = effectiveImage.flatMap { NSImage(contentsOf: $0) }
                enhancementWarning = result.enhancement_warning.map { "超分失败，当前显示原图。\($0)" }
                cacheURL = URL(fileURLWithPath: result.cache); pageRevision = result.revision
                page = result.page
                if page != nil { prepareReadingRegions(cache: cacheURL!) }
                if updateMessage, !busy, message == loadingMessage {
                    if let page {
                        let review = page.blocks.filter(\.needsOCRReview).count
                        message = "已读取本页识别与译文 · \(page.blocks.count - review) 处文字"
                        if review > 0 { message += " · \(review) 处疑似非文字，待核对" }
                    } else { message = "当前翻译方式下尚无本页识别结果" }
                }
            } catch {
                guard !Task.isCancelled, request == pageRequest else { return }
                page = nil; cacheURL = nil; pageRevision = nil; layoutImage = nil
                let detail = "无法读取本页：\(error.localizedDescription)"
                pageLoadError = detail
                self.error = detail
                if updateMessage, !busy, message == loadingMessage { message = "未能读取本页结果" }
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
        guard let layoutImage, let revision = pageRevision else { return }
        let key = layoutImage.path + ":" + revision
        if let saved = readingCache[key] { readingRegions = saved.regions; readingPreserved = saved.preserved ?? [:]; return }
        let request = readingRequest
        readingTask = Task { [weak self] in
            guard let self else { return }
            do {
                let regions = try await ReadingOverlayService.load(service: nativeCore, image: layoutImage, cache: cache, revision: revision)
                guard !Task.isCancelled, request == readingRequest else { return }
                if readingCache.count >= 16 { readingCache.removeAll() }
                readingCache[key] = regions
                readingRegions = regions.regions
                readingPreserved = regions.preserved ?? [:]
            } catch {
                guard !Task.isCancelled, request == readingRequest else { return }
                readingLayoutError = "回填失败：\(error.localizedDescription)"
            }
        }
    }

    func move(_ offset: Int) {
        guard let selected, let i = files.firstIndex(of: selected), files.indices.contains(i + offset) else { return }
        select(files[i + offset])
    }

    func saveEdit(id: String, source: String, translation: String, completion: ((Bool) -> Void)? = nil) {
        guard let block = page?.blocks.first(where: { $0.id == id }) else {
            completion?(false)
            return
        }
        var edit = ReaderEditDraft(block: block)
        edit.source = source
        edit.translation = translation
        let savedTranslation = edit.savedTranslation
        editBlock(id: id, fields: [
            "source": source, "translation": savedTranslation,
            "edited": !savedTranslation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
            "error": "", "vision_source": "", "quality_warnings": []],
            status: edit.sourceChanged && savedTranslation.isEmpty ? "日文已修改，旧译文已清除；点击翻译生成新译文" : "修改已保存", completion: completion)
    }

    func setPreserveBubble(_ keep: Bool, id: String) {
        guard let block = page?.blocks.first(where: { $0.id == id }), block.keepsOriginal != keep else { return }
        let status = keep ? "已保留整个对话框" : "已恢复这个对话框的翻译"
        let selected = self.selected
        editBlock(id: id, fields: ["keep_original": keep], status: status) { succeeded in
            guard succeeded, self.selected == selected else { return }
            self.displayMode = 2
            if !keep, let updated = self.page?.blocks.first(where: { $0.id == id }),
               updated.translation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !updated.error.isEmpty {
                self.message += "，可点击「重新翻译」生成译文"
            }
        }
    }

    func confirmOCR(_ id: String) {
        editBlock(id: id, fields: ["ocr_confirmed": true],
                  status: "已确认为文字，可编辑或重新翻译", completion: nil)
    }

    private func editBlock(id: String, fields: [String: Any], status: String, completion: ((Bool) -> Void)?) {
        guard !busy, !openingFolder, !preparingImage, let cacheURL, let revision = pageRevision else {
            completion?(false)
            return
        }
        let selected = self.selected
        let request = pageRequest
        let layoutImage = self.layoutImage
        busy = true; error = nil; queueTotal = 0; queueDone = 0
        task = Task {
            defer { busy = false; task = nil }
            do {
                var values: [String: Any] = ["cache": cacheURL.path, "block_id": id, "values": fields, "revision": revision]
                if let layoutImage { values["image"] = layoutImage.path }
                let data = try await nativeCore.request("edit", values: values)
                let result = try JSONDecoder().decode(NativeLoadedPage.self, from: data)
                if self.selected == selected, request == pageRequest {
                    pageTask?.cancel()
                    pageRequest = UUID()
                    loadingPage = false
                    resetReadingRegions()
                    pageRevision = result.revision
                    page = result.page
                    prepareReadingRegions(cache: cacheURL)
                } else if self.selected == selected {
                    // A round trip may have loaded this page before the edit committed.
                    // Ask for the current view's image, cache and revision together.
                    reloadPage()
                }
                message = status
                completion?(true)
            } catch is CancellationError {
                if self.selected == selected { reloadPage() }
                message = "已停止保存"
                completion?(false)
            } catch {
                self.error = "保存失败：\(error.localizedDescription)"
                completion?(false)
            }
        }
    }

    func retryBlock(_ id: String) {
        guard !busy, page?.blocks.contains(where: { $0.id == id && !$0.keepsOriginal && !$0.needsOCRReview }) == true else { return }
        run(operation: "translate", blockID: id)
    }

    func run(operation: String = "both", batch: Bool = false, blockID: String? = nil) {
        guard !busy, !openingFolder, !preparingImage, let selected else { return }
        let jobConnection = connection
        var options: [String: Any] = ["mode": processingMode, "model": model, "provider": jobConnection.provider,
            "base_url": jobConnection.baseURL, "key": jobConnection.apiKey]
        if let blockID { options["block_id"] = blockID; options["retry"] = true }
        defaults.set(model, forKey: "translationModel." + jobConnection.identity)
        runQueue(batch ? files : [selected], operation: operation, options: options)
    }

    var canEnhanceChapter: Bool {
        !busy && !preparingImage && folderRequest == nil && selected != nil && !files.isEmpty
    }

    func enhanceChapter() {
        guard canEnhanceChapter else { return }
        if srEngine == "off" {
            srEngine = "metalfx"
            srPreset = "standard"
            srStatus = ""
        }
        runQueue(files, operation: "enhance")
    }

    func defaultChapterExportOptions() -> ChapterExport.Options {
        ChapterExport.Options(pages: IndexSet(integersIn: 1..<(files.count + 1)),
                              folderName: (try? ChapterExport.validatedFolderName(folderName)) ?? "漫画",
                              directory: ChapterExport.defaultDirectory(project: root, defaults: defaults))
    }

    func exportChapter(_ options: ChapterExport.Options) {
        guard canEnhanceChapter else { return }
        guard let first = options.pages.first, let last = options.pages.last, first >= 1, last <= files.count else {
            error = "请选择第 1 页到第 \(files.count) 页之间要导出的页码。"
            return
        }
        guard options.directory.isFileURL, !options.directory.path.isEmpty,
              !options.directory.path.utf8.contains(0) else {
            error = "请选择导出文件的保存位置。"
            return
        }
        let exportFolderName: String
        do { exportFolderName = try ChapterExport.validatedFolderName(options.folderName) }
        catch {
            self.error = error.localizedDescription
            return
        }
        let directory = options.directory.standardizedFileURL
        let isCustomDirectory = directory.path != ChapterExport.defaultDirectory(project: root, defaults: defaults).path
        let pages = options.pages.map { (number: $0, file: files[$0 - 1]) }
        let digits = max(3, String(files.count).count)
        let mode = processingMode
        let useOriginal = srEngine == "off"
        var enhancement = srSettings
        if useOriginal { enhancement["engine"] = "metalfx"; enhancement["preset"] = "standard" }

        busy = true; exportingChapter = true; error = nil; lastChapterExport = nil
        queueTotal = pages.count; queueDone = 0
        message = "正在准备导出 \(pages.count) 页漫画…"
        task = Task {
            defer { busy = false; exportingChapter = false; task = nil }
            var currentPage: URL?
            do {
                try Task.checkCancellation()
                let staging = try ChapterExport.stage(in: directory)
                defer { try? FileManager.default.removeItem(at: staging) }
                var omitted = 0
                for (index, entry) in pages.enumerated() {
                    try Task.checkCancellation()
                    let input = entry.file
                    let pageNumber = entry.number
                    currentPage = input
                    message = "正在导出第 \(pageNumber) 页（\(input.lastPathComponent)）…"
                    let data = try await nativeCore.request("chapter_export_page", values: ["image": input.path,
                        "mode": mode, "super_resolution": enhancement, "prefer_original": useOriginal])
                    try Task.checkCancellation()
                    let page = try JSONDecoder().decode(ChapterExport.Page.self, from: data)
                    let number = String(pageNumber)
                    let name = String(repeating: "0", count: digits - number.count) + number + ".png"
                    omitted += try await ChapterExport.write(page, to: staging.appendingPathComponent(name))
                    try Task.checkCancellation()
                    queueDone = index + 1
                }
                try Task.checkCancellation()
                currentPage = nil
                lastChapterExport = try ChapterExport.finish(staging, in: directory, folderName: exportFolderName)
                if isCustomDirectory { ChapterExport.rememberDirectory(directory, defaults: defaults) }
                message = "已导出 \(pages.count) 页超分漫画"
                if omitted > 0 { message += " · \(omitted) 处缺译文或暂无法排版，已保留原文" }
            } catch is CancellationError {
                message = "已取消导出"
            } catch {
                let pageName = currentPage.map { "「\($0.lastPathComponent)」" } ?? ""
                self.error = "\(pageName)导出失败：\(error.localizedDescription)"
                message = "导出未完成"
            }
        }
    }

    private func runQueue(_ urls: [URL], operation: String, options: [String: Any] = [:]) {
        let enhancing = operation == "enhance"
        if !enhancing {
            for url in urls { pageProcessingErrors.removeValue(forKey: url) }
        }
        var values = options
        values["images"] = urls.map(\.path)
        values["operation"] = operation
        values["super_resolution"] = srSettings
        busy = true; error = nil; queueTotal = urls.count; queueDone = 0
        if enhancing { message = "正在准备整章超分…" }
        let request = UUID(); operationRequest = request
        task = Task { [self] in
            var startedImages = Set<String>()
            defer {
                // No newer processing task can start while this one owns busy.
                busy = false; task = nil
                if Task.isCancelled { message = "已停止；已完成的结果已保存" }
                if !enhancing, request == operationRequest || selected.map({ startedImages.contains($0.path) }) == true {
                    reloadPage()
                }
            }
            do {
                let data = try await nativeCore.request("process", values: values) { [weak self] event in
                    // Rust announces a page before writing its cache. Checkpoints can
                    // arrive after Stop, or be dropped while the event channel is full.
                    if event["event"] as? String == "page_started", let path = event["image"] as? String {
                        startedImages.insert(path)
                    }
                    guard let self, request == operationRequest else { return }
                    switch event["event"] as? String {
                    case "progress": if let text = event["message"] as? String { message = text }
                    case "page_started":
                        if let path = event["image"] as? String {
                            let action = enhancing ? "超分" : "处理"
                            message = "正在\(action) \(URL(fileURLWithPath: path).lastPathComponent)…"
                        }
                    case "page_finished":
                        queueDone = event["done"] as? Int ?? queueDone
                        if let result = event["page"] as? [String: Any] {
                            if !enhancing, let text = result["error"] as? String,
                               let path = result["image"] as? String {
                                let url = URL(fileURLWithPath: path)
                                pageProcessingErrors[url] = text
                                if url == self.selected { self.error = text }
                            }
                            if result["image"] as? String == self.selected?.path,
                               !enhancing || result["ok"] as? Bool == true { reloadPage() }
                        }
                    default: break
                    }
                }
                try Task.checkCancellation()
                guard request == operationRequest else { return }
                let result = try JSONSerialization.jsonObject(with: data) as? [String: Any]
                guard let pages = result?["pages"] as? [[String: Any]] else {
                    throw NativeCoreService.Failure(message: "处理服务返回了无效的页面结果。")
                }
                queueDone = pages.count
                if enhancing {
                    let failures = pages.filter { $0["ok"] as? Bool != true }
                    let skipped = pages.filter { ($0["result"] as? [String: Any])?["skipped"] as? Bool == true }.count
                    let enhanced = pages.count - failures.count - skipped
                    let summary = "\(enhanced) 页增强就绪 · \(skipped) 页已达目标"
                    if let first = failures.first, let path = first["image"] as? String, let detail = first["error"] as? String {
                        self.error = "「\(URL(fileURLWithPath: path).lastPathComponent)」超分失败 · \(detail)"
                    }
                    message = failures.isEmpty ? "整章超分完成 · \(summary)" : "整章超分结束 · \(summary) · \(failures.count) 页失败"
                } else {
                    let failures = pages.reduce(0) { total, page in
                        total + ((page["ok"] as? Bool == true) ? ((page["result"] as? [String: Any])?["issues"] as? Int ?? 0) : 1)
                    }
                    message = failures > 0 ? "处理结束，有 \(failures) 处问题；请查看提示后重试" : "处理完成 · \(queueDone) 页 · 结果已保存到本机"
                }
            } catch is CancellationError { if request == operationRequest { message = "已停止；已完成的结果已保存" } }
            catch {
                if request == operationRequest {
                    let detail = error.localizedDescription
                    if !enhancing, urls.count == 1, let url = urls.first { pageProcessingErrors[url] = detail }
                    self.error = detail
                    message = "处理未完成；已保存的结果保留，可重试"
                }
            }
        }
    }

    func cancel() {
        if exportingChapter {
            message = "正在停止导出…"
            task?.cancel()
            return
        }
        if busy {
            message = "正在停止处理…"
            operationRequest = UUID()
            task?.cancel()
            // Page navigation and its overlay load belong to the reader, not the queued job.
            return
        }
        if preparingImage {
            message = "已停止前置增强；当前页使用原图"
            reloadPage(useOriginalImage: true)
            return
        }
        pageTask?.cancel(); readingTask?.cancel()
        pageRequest = UUID(); readingRequest = UUID(); preparingImage = false; loadingPage = false
    }
    func shutdown() {
        folderRequest = nil; connectionRequest = nil
        pageTask?.cancel(); readingTask?.cancel(); task?.cancel(); modelTimer?.invalidate()
        nativeCore.shutdown()
    }

}

#if !KOMA_REVIEW_HARNESS
@main struct KomaReaderApp: App {
    @StateObject private var reader = Reader()
    var body: some Scene {
        WindowGroup("Koma Rust · 本地漫画翻译") { ContentView(reader: reader).tint(ReaderPalette.accent) }
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

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
    var block_id: String?
}

@MainActor final class Reader: ObservableObject {
    static let enhancementTargets = [2560, 3840]

    @Published var files: [URL] = []
    @Published var selected: URL? {
        didSet { if selected != oldValue { blockSelection = .awaitingPage } }
    }
    @Published var page: PageResult? {
        didSet {
            presentationCache = nil
            if let page { reconcileBlockSelection(on: page) }
        }
    }
    @Published var readingRegions: [ReadingRegion]? { didSet { presentationCache = nil } }
    @Published var readingPreserved: [String: String] = [:]
    private var presentationCache: [ReadingPresentation]?
    @Published private var typography = ChapterTypography() { didSet { presentationCache = nil } }
    private var typographyURL: URL?
    private var originalPixelSize: CGSize?
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
    @Published var cropSession: ReaderCropSession?
    private enum BlockSelection {
        case awaitingPage, cleared, block(String)
    }
    @Published private var blockSelection: BlockSelection = .awaitingPage
    var selectedBlock: String? {
        get {
            if case let .block(id) = blockSelection { return id }
            return nil
        }
        set { blockSelection = newValue.map(BlockSelection.block) ?? .cleared }
    }

    func clearBlockSelection() {
        guard cropSession == nil else { return }
        selectedBlock = nil
    }

    private func reconcileBlockSelection(on page: PageResult) {
        switch blockSelection {
        case .cleared: return
        case .block(let id) where page.blocks.contains(where: { $0.id == id }): return
        default: blockSelection = page.blocks.first.map { .block($0.id) } ?? .awaitingPage
        }
    }
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
    @Published private(set) var releasingModel = false
    @Published private(set) var modelReleaseNotice: String?
    @Published private(set) var modelReleaseError: String?
    private struct ModelRelease {
        let connection: TranslationConnection
        let model: String
        let explicit: Bool
    }
    private var failedModelRelease: ModelRelease?
    private var modelReleaseTask: Task<Void, Never>?
    private var modelReleaseRequest = UUID()
    private var shuttingDown = false
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
    var frameEditIdentity: String? { editCacheIdentity.map { $0 + "|" + (pageRevision ?? "") } }
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
    private let runtimeRoot: URL?
    var platformRoot: URL { root.appendingPathComponent("macos") }
    var samplesDirectory: URL { root.appendingPathComponent("samples", isDirectory: true) }
    let cacheDir: URL
    private lazy var nativeCore: NativeCoreService = {
        let seed = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/KomaReader/pages")
        return NativeCoreService(root: root, cache: cacheDir,
                                 assets: cacheDir.deletingLastPathComponent().appendingPathComponent("reading-native-v1"),
                                 seed: FileManager.default.fileExists(atPath: seed.path) ? seed : nil,
                                 runtimeRoot: runtimeRoot)
    }()

    // The candidate bundle identifier already isolates .standard. Foundation
    // rejects using the application's own identifier as an explicit suite.
    init(root: URL? = nil, cacheDir: URL? = nil,
         defaults: UserDefaults = .standard,
         configurationStore: TranslationLocalConfigStore? = nil) {
        let configured = Bundle.main.object(forInfoDictionaryKey: "ProjectRoot") as? String
        let resourceFolder = Bundle.main.object(forInfoDictionaryKey: "KomaRuntimeFolder") as? String
        let bundled = resourceFolder.flatMap { Bundle.main.resourceURL?.appendingPathComponent($0, isDirectory: true) }
        let namespace = (Bundle.main.object(forInfoDictionaryKey: "CacheNamespace") as? String) ?? "KomaReaderRust"
        let support = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/" + namespace)
        self.root = root ?? bundled ?? URL(fileURLWithPath: configured ?? FileManager.default.currentDirectoryPath)
        self.cacheDir = cacheDir ?? support.appendingPathComponent("pages")
        self.runtimeRoot = root == nil && bundled != nil ? support.appendingPathComponent("runtime") : nil
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
        guard !shuttingDown, !busy, availableModels.contains(value), value != model else { return }
        do {
            try persistModel(value)
            releaseModelIfChanging(to: connection, model: value)
            model = value
            defaults.set(value, forKey: "translationModel." + connection.identity)
        } catch { connectionError = error.localizedDescription }
    }

    private func releaseModelIfChanging(to next: TranslationConnection, model nextModel: String) {
        guard connection.identity != next.identity || model != nextModel else { return }
        modelReleaseNotice = nil
        guard !model.isEmpty else { return }
        enqueueModelRelease(ModelRelease(connection: connection, model: model, explicit: false))
    }

    func releaseCurrentModel() {
        guard !shuttingDown, !busy, !releasingModel, !model.isEmpty else { return }
        enqueueModelRelease(ModelRelease(connection: connection, model: model, explicit: true))
    }

    func retryModelRelease() {
        guard !shuttingDown, !busy, !releasingModel, let failedModelRelease else { return }
        enqueueModelRelease(failedModelRelease)
    }

    private func enqueueModelRelease(_ target: ModelRelease) {
        let previous = modelReleaseTask
        let request = UUID()
        modelReleaseRequest = request
        releasingModel = true
        modelReleaseNotice = nil
        let config = target.connection
        let values: [String: Any] = ["provider": config.provider, "base_url": config.baseURL,
                                     "key": config.apiKey, "model": target.model]
        // Explicit actions are admitted only while idle. Submit now so even
        // an immediate Quit reaches Rust after this request.
        let explicitReply = target.explicit ? nativeCore.submitExplicitUnload(values) : nil
        modelReleaseTask = Task {
            await previous?.value
            defer {
                if request == modelReleaseRequest { releasingModel = false; modelReleaseTask = nil }
            }
            do {
                if let explicitReply {
                    struct Reply: Decodable { let released: Bool }
                    var iterator = explicitReply.makeAsyncIterator()
                    guard let data = try await iterator.next() else { throw CancellationError() }
                    let released = try JSONDecoder().decode(Reply.self, from: data).released
                    modelReleaseNotice = released ? "已释放「\(target.model)」，下次翻译时会重新加载。" : "「\(target.model)」当前未加载。"
                } else {
                    _ = try await nativeCore.requestIfRunning("unload_model", values: values)
                }
                if failedModelRelease?.connection.identity == config.identity, failedModelRelease?.model == target.model {
                    failedModelRelease = nil
                    modelReleaseError = nil
                }
            } catch {
                failedModelRelease = target
                modelReleaseError = "释放 \(config.name) 模型「\(target.model)」失败：\(error.localizedDescription)"
            }
        }
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
        guard !shuttingDown, !busy else { throw ConnectionError("请等待当前任务结束后再切换接口。") }
        guard models.contains(model) else { throw ConnectionError("请先获取并选择一个模型。") }
        if let configurationLoadError { throw ConnectionError(configurationLoadError) }
        let config = try config.normalized()
        let updated = try localConfiguration.applying(config, model: model, rememberKey: rememberKey)
        try connectionConfigStore.save(updated)
        releaseModelIfChanging(to: config, model: model)
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
        releaseUnusedLocalOllama()
    }

    func startup() {
        guard !started else { return }
        started = true
        startEngine()
        modelTimer = Timer.scheduledTimer(withTimeInterval: 10, repeats: true) { [weak self] _ in
            Task { @MainActor in _ = await self?.refreshModels() }
        }
        if let folder = defaults.string(forKey: "folder") {
            // Older releases stored the full path to the copy of Koma.app that was last opened.
            let sampleSuffix = ".app/Contents/Resources/" + root.lastPathComponent + "/samples"
            let bundledSample = runtimeRoot != nil && (folder == "samples" || folder.hasSuffix(sampleSuffix))
            let url = bundledSample ? samplesDirectory : URL(fileURLWithPath: folder)
            loadFolder(url, restore: true)
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
        guard !usesLocalOllama, !shuttingDown else { return }
        let release = modelReleaseTask
        Task {
            await release?.value
            guard !shuttingDown, !usesLocalOllama, localOllamaProbes.isEmpty else { return }
            _ = try? await nativeCore.requestIfRunning("release_engine")
        }
    }

    private var usesLocalOllama: Bool {
        connection.provider == "ollama" && connection.baseURL == TranslationConnection.defaultURL("ollama")
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
        guard !shuttingDown, !busy, connectionRequest == nil else { return engineReady }
        let snapshot = connection
        let request = UUID()
        connectionRequest = request
        defer { if request == connectionRequest { connectionRequest = nil } }
        do {
            let models = try await discoverModels(snapshot, allowLocalStart: allowLocalStart, requireCurrentConnection: true)
            guard request == connectionRequest, snapshot == connection, !busy else { return false }
            let selectedModel = models.contains(model) ? model : (models.first ?? "")
            if !selectedModel.isEmpty { try persistModel(selectedModel) }
            releaseModelIfChanging(to: connection, model: selectedModel)
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

    private func isBundledSampleDirectory(_ url: URL) -> Bool {
        guard runtimeRoot != nil else { return false }
        return url.resolvingSymlinksInPath().standardizedFileURL.path == samplesDirectory.resolvingSymlinksInPath().standardizedFileURL.path
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
                let bundledSample = isBundledSampleDirectory(url)
                defaults.set(bundledSample ? "samples" : url.path, forKey: "folder")
                let last = defaults.string(forKey: "lastPage").map { bundledSample ? URL(fileURLWithPath: $0).lastPathComponent : $0 }
                let initial = restore ? (images.first { (bundledSample ? $0.lastPathComponent : $0.path) == last } ?? images[0]) : images[0]
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
        cancelCrop()
        effectiveImage = nil; layoutImage = nil
        selected = url; error = nil
        originalPixelSize = ChapterTypography.originalSize(at: url)
        let folder = url.deletingLastPathComponent()
        let bundledSample = isBundledSampleDirectory(folder)
        let styleFile = bundledSample
            ? cacheDir.deletingLastPathComponent().appendingPathComponent("sample-typesetting.json")
            : folder.appendingPathComponent(ChapterTypography.filename)
        if typographyURL != styleFile {
            typographyURL = styleFile
            do { typography = try ChapterTypography.read(from: styleFile) }
            catch {
                typography = ChapterTypography()
                self.error = "无法读取章节排版：\(error.localizedDescription)"
            }
        }
        image = NSImage(contentsOf: url)
        if image == nil { error = "无法解码这张图片：\(url.lastPathComponent)" }
        page = nil; cacheURL = nil; pageRevision = nil
        reloadPage(updateMessage: !busy)
        defaults.set(bundledSample ? url.lastPathComponent : url.path, forKey: "lastPage")
    }

    private func resetReadingRegions() {
        readingTask?.cancel()
        readingRequest = UUID()
        readingRegions = nil; readingPreserved = [:]; readingLayoutError = nil
    }

    func reloadPage(updateMessage: Bool = false, useOriginalImage: Bool = false, retryChangedRevision: Bool = true) {
        cancelCrop()
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
                if page != nil {
                    prepareReadingRegions(cache: cacheURL!, useOriginalImage: useOriginalImage, retryChangedRevision: retryChangedRevision)
                }
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
        let styles = typographyPageKey.map { typography.styles(for: page, key: $0) } ?? [:]
        let scale = typesettingScale(page)
        let items = ReadingLayout.presentations(page: page, regions: readingRegions, styles: styles,
                                                sourcePixelScale: scale.width, sourceYScale: scale.height)
        presentationCache = items
        return items
    }

    private var typographyPageKey: String? {
        guard let selected, let cacheURL else { return nil }
        return ChapterTypography.pageKey(image: selected, cache: cacheURL)
    }

    var chapterTypography: ReadingTextStyle { typography.defaults }

    func typographyOverride(for block: TextBlock) -> ReadingTextStyle {
        guard let page, let key = typographyPageKey else { return ReadingTextStyle() }
        return typography.override(for: block, on: page, key: key)
    }

    func resolvedTypography(for block: TextBlock) -> ReadingTextStyle {
        guard let page, let key = typographyPageKey else { return chapterTypography }
        return typography.resolved(for: block, on: page, key: key)
    }

    func actualTypeset(for block: TextBlock) -> ReadingTypeset? {
        readingPresentations.first { $0.region.block_ids.contains(block.id) }?.layout(for: block.id)
    }

    private func typesettingScale(_ page: PageResult) -> CGSize {
        guard let size = originalPixelSize else { return CGSize(width: 1, height: 1) }
        return CGSize(width: Double(page.width) / size.width, height: Double(page.height) / size.height)
    }

    func editingFrame(for block: TextBlock, in region: ReadingRegion, page: PageResult) -> CGRect? {
        let scale = typesettingScale(page)
        if let value = typographyOverride(for: block).textFrame {
            return value.applying(CGAffineTransform(scaleX: scale.width, y: scale.height))
        }
        if let slot = ReadingLayout.memberSlot(for: block.id, region: region) { return slot }
        if let run = readingPresentations.first(where: { $0.id == region.id })?.runs.first(where: { $0.blockIDs.contains(block.id) }) {
            let layout: ReadingTypeset?
            if run.blockIDs.count == 1 { layout = run.layout }
            else {
                let members = run.blockIDs.compactMap { id in page.blocks.first { $0.id == id } }
                layout = ReadingRun.paragraphs(of: run.layout, members: members)?.first { $0.blockIDs == [block.id] }?.layout
            }
            if let layout {
                let visible = layout.bounds.insetBy(dx: -layout.fontSize * 0.4, dy: -layout.fontSize * 0.4)
                return region.block_ids.count == 1 ? (region.rectangle ?? visible).union(visible) : visible
            }
        }
        if region.block_ids.count == 1 { return region.rectangle }
        guard block.box.count == 4, block.box.allSatisfy(\.isFinite) else { return nil }
        return CGRect(x: block.box[0], y: block.box[1], width: block.box[2] - block.box[0], height: block.box[3] - block.box[1])
    }

    private func originalFrame(_ frame: CGRect, page: PageResult) -> CGRect {
        let scale = typesettingScale(page)
        return frame.applying(CGAffineTransform(scaleX: 1 / scale.width, y: 1 / scale.height))
    }

    func previewFrame(_ frame: CGRect, for block: TextBlock, in region: ReadingRegion) -> ReadingPresentation? {
        guard let page, let key = typographyPageKey else { return nil }
        var styles = typography.styles(for: page, key: key)
        styles[block.id, default: ReadingTextStyle()].textFrame = originalFrame(frame, page: page)
        let scale = typesettingScale(page)
        return ReadingLayout.presentations(page: page, regions: [region], styles: styles,
                                          sourcePixelScale: scale.width, sourceYScale: scale.height).first
    }

    // Several individually overflowing translations can share a single repair. Allow
    // fixing them one by one, but never show that repair until all members actually fit.
    func canStageFrame(_ frame: CGRect, for block: TextBlock, sharing region: ReadingRegion) -> Bool {
        var member = region
        member.block_ids = [block.id]
        member.member_slots = nil
        return previewFrame(frame, for: block, in: member) != nil
    }

    @discardableResult func setTextFrame(_ frame: CGRect?, for block: TextBlock) -> Bool {
        guard let page, let key = typographyPageKey,
              page.blocks.contains(where: { $0.id == block.id && $0.box == block.box }) else { return false }
        let value = frame.map { originalFrame($0, page: page) }
        guard typographyOverride(for: block).textFrame != value else { return true }
        return saveTypography {
            var override = $0.override(for: block, on: page, key: key)
            override.textFrame = value
            $0.set(override, for: block, on: page, key: key)
        }
    }

    func setTypographyOverride(_ style: ReadingTextStyle, for block: TextBlock) {
        guard let page, let key = typographyPageKey, typographyOverride(for: block) != style,
              page.blocks.contains(where: { $0.id == block.id && $0.box == block.box }) else { return }
        saveTypography { $0.set(style, for: block, on: page, key: key) }
    }

    func resetTypographyOverride(for block: TextBlock) {
        setTypographyOverride(ReadingTextStyle(textFrame: typographyOverride(for: block).textFrame), for: block)
    }

    @discardableResult func setChapterTypography(_ style: ReadingTextStyle) -> Bool {
        guard chapterTypography != style else { return true }
        return saveTypography { $0.defaults = style }
    }

    @discardableResult private func saveTypography(_ change: (inout ChapterTypography) -> Void) -> Bool {
        guard !busy, !openingFolder, let typographyURL else { return false }
        do {
            typography = try ChapterTypography.update(at: typographyURL, change: change)
            displayMode = 2
            return true
        } catch {
            self.error = "章节排版未保存：\(error.localizedDescription)"
            return false
        }
    }

    func reflowCurrentPageTypography() {
        guard !busy, !openingFolder, page != nil, readingRegions != nil else { return }
        presentationCache = nil
        displayMode = 2
        let missing = unreadableOverlayCount
        message = missing == 0 ? "已按现有译文重排本页" : "已重排本页 · \(missing) 处无法回填，已保留原图"
    }

    var unreadableOverlayCount: Int {
        guard let page else { return 0 }
        let shown = Set(readingPresentations.flatMap { $0.region.block_ids })
        return page.blocks.filter { $0.needsReadingOverlay && !shown.contains($0.id) && readingPreserved[$0.id] == nil }.count
    }

    private func prepareReadingRegions(cache: URL, useOriginalImage: Bool = false, retryChangedRevision: Bool = true) {
        guard let layoutImage, let revision = pageRevision else { return }
        let key = cache.path + ":" + revision
        if let saved = readingCache[key] { readingRegions = saved.regions; readingPreserved = saved.preserved ?? [:]; return }
        let request = readingRequest, selected = self.selected
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
                guard !Task.isCancelled, request == readingRequest, selected == self.selected, revision == pageRevision else { return }
                if retryChangedRevision, (error as? NativeCoreService.Failure)?.code == "page_revision_changed" {
                    // A new checkpoint landed between the page and overlay requests.
                    reloadPage(useOriginalImage: useOriginalImage, retryChangedRevision: false)
                    return
                }
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

    var canBeginCrop: Bool {
        !shuttingDown && !busy && !openingFolder && !preparingImage && !loadingPage
            && cacheURL != nil && selected != nil && cropSession == nil
    }

    func beginCrop() {
        guard canBeginCrop, let selected, let cacheURL, let original = NSImage(contentsOf: selected),
              let canvas = page.map({ CGSize(width: $0.width, height: $0.height) }) ?? ChapterTypography.originalSize(at: selected) else { return }
        let target = page?.blocks.first { $0.id == selectedBlock && !$0.keepsOriginal }?.id
        cropSession = ReaderCropSession(image: selected, cache: cacheURL, revision: pageRevision,
                                        original: original, canvas: canvas, placement: target == nil ? .new : .prepend,
                                        target: target)
        error = nil
    }

    func cancelCrop() {
        guard let session = cropSession, session.phase != .applying else { return }
        cropSession = nil
        if session.phase == .recognizing { task?.cancel() }
    }

    func dismissCropReview() {
        let reload = cropSession?.outdated == true
        cancelCrop()
        if reload { reloadPage(updateMessage: true) }
    }

    func reselectCrop() {
        guard cropSession?.phase == .reviewing else { return }
        cropSession?.box = []
        cropSession?.source = ""
        cropSession?.issue = nil
        cropSession?.phase = .selecting
    }

    private func cropRequest(_ session: ReaderCropSession) -> [String: Any] {
        var request: [String: Any] = ["image": session.image.path, "cache": session.cache.path, "box": session.box]
        if let revision = session.revision { request["revision"] = revision }
        return request
    }

    func previewCrop(_ box: [Int]) {
        guard !busy, var session = cropSession, session.phase == .selecting, box.count == 4 else { return }
        session.box = box
        session.phase = .recognizing
        cropSession = session
        busy = true
        task = Task {
            defer { busy = false; task = nil }
            do {
                struct Preview: Decodable { let source: String; let box: [Int]; let revision: String? }
                let data = try await nativeCore.request("ocr_crop_preview", values: cropRequest(session))
                let result = try JSONDecoder().decode(Preview.self, from: data)
                guard !Task.isCancelled, cropSession?.id == session.id, result.revision == session.revision,
                      result.box == box else {
                    if cropSession?.id == session.id { cancelCrop() }
                    return
                }
                cropSession?.source = result.source
                cropSession?.phase = .reviewing
            } catch {
                guard !Task.isCancelled, cropSession?.id == session.id else { return }
                if (error as? NativeCoreService.Failure)?.code == "page_revision_changed" {
                    cropSession?.outdated = true
                    cropSession?.issue = "本页内容已更新，请重新载入后圈选。"
                } else {
                    cropSession?.issue = "识别失败，可重选范围或手动输入日文。\(error.localizedDescription)"
                }
                cropSession?.phase = .reviewing
            }
        }
    }

    func applyCrop(translation: String) {
        guard !busy, let session = cropSession, session.phase == .reviewing, !session.outdated, session.image == selected,
              session.cache == cacheURL, session.revision == pageRevision else { return }
        let source = session.source.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !source.isEmpty else { return }
        var values = cropRequest(session)
        values["placement"] = session.placement.rawValue
        values["source"] = source
        if session.placement != .new {
            guard let id = session.target, page?.blocks.contains(where: { $0.id == id && !$0.keepsOriginal }) == true else { return }
            values["block_id"] = id
        }
        let confirmed = translation.trimmingCharacters(in: .whitespacesAndNewlines)
        if !confirmed.isEmpty { values["translation"] = confirmed }
        cropSession?.issue = nil
        cropSession?.phase = .applying
        busy = true
        task = Task {
            defer { busy = false; task = nil }
            do {
                let data = try await nativeCore.request("ocr_crop_apply", values: values)
                let result = try JSONDecoder().decode(NativeLoadedPage.self, from: data)
                cropSession = nil
                guard self.selected == session.image, cacheURL == session.cache else { return }
                if let id = result.block_id { selectedBlock = id }
                pageTask?.cancel()
                pageRequest = UUID()
                loadingPage = false
                resetReadingRegions()
                pageRevision = result.revision
                page = result.page
                displayMode = confirmed.isEmpty ? 1 : 2
                prepareReadingRegions(cache: session.cache)
                message = confirmed.isEmpty ? "已保存圈选日文，可核对译文或重新翻译。" : "已保存圈选日文与译文"
            } catch {
                if let failure = error as? NativeCoreService.Failure, failure.code == "page_revision_changed" {
                    cropSession?.outdated = true
                    cropSession?.issue = "本页已有其他修改。可先复制这里的日文或译文，再关闭并重新载入。"
                    cropSession?.phase = .reviewing
                } else {
                    cropSession?.issue = "保存失败：\(error.localizedDescription)"
                    cropSession?.phase = .reviewing
                }
            }
        }
    }

    func retryBlock(_ id: String) {
        guard !busy, page?.blocks.contains(where: { $0.id == id && !$0.keepsOriginal && !$0.needsOCRReview }) == true else { return }
        run(operation: "translate", blockID: id)
    }

    func run(operation: String = "both", batch: Bool = false, blockID: String? = nil) {
        guard !shuttingDown, !busy, !openingFolder, !preparingImage, let selected else { return }
        let jobConnection = connection
        var options: [String: Any] = ["mode": processingMode, "model": model, "provider": jobConnection.provider,
            "base_url": jobConnection.baseURL, "key": jobConnection.apiKey]
        if let blockID {
            guard !batch, let cacheURL, let pageRevision else {
                error = "请重新载入本页，再选择要重译的对白。"
                reloadPage()
                return
            }
            options["block_id"] = blockID
            options["retry"] = true
            options["cache"] = cacheURL.path
            options["revision"] = pageRevision
        }
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
                              directory: ChapterExport.defaultDirectory(project: runtimeRoot ?? root, defaults: defaults))
    }

    var chapterExportEnhancement: ChapterExport.Enhancement {
        .init(engine: srEngine, target: srTarget, axis: srAxis, preset: srPreset)
    }

    func checkChapterExportPage(_ image: URL, source: ExportSource,
                                enhancement: ChapterExport.Enhancement) async throws -> ExportPageCheck {
        guard let data = try await nativeCore.requestIfRunning("export_preflight", values: [
            "image": image.path, "mode": source.rawValue, "super_resolution": enhancement.settings
        ]) else {
            throw NativeCoreService.Failure(message: "页面服务尚未运行。请返回阅读器重新载入一页后再检查，或在这里仍尝试导出。")
        }
        return try JSONDecoder().decode(ExportPageCheck.self, from: data)
    }

    func exportChapter(_ options: ChapterExport.Options, reviewed: ExportReview? = nil,
                       enhancement: ChapterExport.Enhancement? = nil,
                       completion: ((ChapterExport.Outcome) -> Void)? = nil) {
        guard canEnhanceChapter else {
            completion?(.failed("请等待当前处理完成后再导出。"))
            return
        }
        func reject(_ reason: String) {
            error = reason
            completion?(.failed(reason))
        }
        cancelCrop()
        guard let first = options.pages.first, let last = options.pages.last, first >= 1, last <= files.count else {
            reject("请选择第 1 页到第 \(files.count) 页之间要导出的页码。")
            return
        }
        guard options.directory.isFileURL, !options.directory.path.isEmpty,
              !options.directory.path.utf8.contains(0) else {
            reject("请选择导出文件的保存位置。")
            return
        }
        let exportFolderName: String
        do { exportFolderName = try ChapterExport.validatedFolderName(options.folderName) }
        catch { reject(error.localizedDescription); return }
        if let reviewed, reviewed.selection.pages != options.pages || !reviewed.missing.isEmpty {
            reject("所选页面与检查结果不一致，请重新检查。")
            return
        }
        let directory = options.directory.standardizedFileURL
        let isCustomDirectory = directory.path != ChapterExport.defaultDirectory(project: runtimeRoot ?? root, defaults: defaults).path
        let pages = options.pages.map { (number: $0, file: files[$0 - 1]) }
        let exportTypography = typography
        let digits = max(3, String(files.count).count)
        let mode = reviewed?.selection.source.rawValue ?? processingMode
        let enhancement = enhancement ?? chapterExportEnhancement
        let fingerprints = reviewed?.fingerprints ?? [:]

        busy = true; exportingChapter = true; error = nil; lastChapterExport = nil
        queueTotal = pages.count; queueDone = 0
        message = "正在准备导出 \(pages.count) 页漫画…"
        task = Task {
            defer { busy = false; exportingChapter = false; task = nil }
            var currentPage: (number: Int, file: URL)?
            do {
                try Task.checkCancellation()
                let staging = try ChapterExport.stage(in: directory)
                defer { try? FileManager.default.removeItem(at: staging) }
                let exportID = UUID().uuidString
                defer {
                    // A cancelled export must still release its last enhanced image.
                    Task { try? await nativeCore.request("release_export", values: ["export_id": exportID]) }
                }
                var omissions: [Int: Int] = [:]
                for (index, entry) in pages.enumerated() {
                    try Task.checkCancellation()
                    let input = entry.file
                    let pageNumber = entry.number
                    currentPage = entry
                    message = "正在导出第 \(pageNumber) 页（\(input.lastPathComponent)）…"
                    var values: [String: Any] = ["image": input.path, "mode": mode, "super_resolution": enhancement.settings,
                                                 "prefer_original": enhancement.preferOriginal, "export_id": exportID]
                    if let expected = fingerprints[pageNumber] { values["expected_fingerprint"] = expected }
                    let data = try await nativeCore.request("chapter_export_page", values: values)
                    try Task.checkCancellation()
                    let page = try JSONDecoder().decode(ChapterExport.Page.self, from: data)
                    let number = String(pageNumber)
                    let name = String(repeating: "0", count: digits - number.count) + number + ".png"
                    let returnedCache = page.cache.map { URL(fileURLWithPath: $0) }
                    let verifyOriginal = fingerprints[pageNumber] != nil &&
                        URL(fileURLWithPath: page.effective_image).standardizedFileURL == input.standardizedFileURL
                    if verifyOriginal && returnedCache == nil { throw CocoaError(.fileReadCorruptFile) }
                    let cache = try returnedCache ?? cacheFor(input, mode: mode)
                    let originalDigest = verifyOriginal ? cache.deletingPathExtension().lastPathComponent : nil
                    let key = ChapterTypography.pageKey(image: input, cache: cache)
                    let styles = exportTypography.styles(for: page.page, key: key)
                    let size = verifyOriginal ? nil : ChapterTypography.originalSize(at: input)
                    let scale = ChapterTypography.pixelScale(for: page.page, originalWidth: size.map { Double($0.width) })
                    let omitted = try await ChapterExport.write(page, to: staging.appendingPathComponent(name),
                                                                styles: styles, sourcePixelScale: scale,
                                                                sourceYScale: size.map { Double(page.page.height) / $0.height },
                                                                expectedOriginalDigest: originalDigest)
                    try Task.checkCancellation()
                    if omitted > 0 { omissions[pageNumber] = omitted }
                    queueDone = index + 1
                }
                try Task.checkCancellation()
                currentPage = nil
                let output = try ChapterExport.finish(staging, in: directory, folderName: exportFolderName)
                lastChapterExport = output
                if isCustomDirectory { ChapterExport.rememberDirectory(directory, defaults: defaults) }
                message = "已导出 \(pages.count) 页超分漫画"
                if !omissions.isEmpty {
                    message += " · \(omissions.count) 页共 \(omissions.values.reduce(0, +)) 处未回填，已保留原图"
                }
                completion?(.completed(directory: output, omissions: omissions))
            } catch is CancellationError {
                message = "已取消导出"
                completion?(.cancelled)
            } catch let failure as NativeCoreService.Failure where failure.code == "page_revision_changed" {
                if let currentPage {
                    message = "第 \(currentPage.number) 页结果已变化，请重新核对后导出。"
                    completion?(.changed(page: currentPage.number))
                }
            } catch {
                let pageName = currentPage.map { "第 \($0.number) 页「\($0.file.lastPathComponent)」" } ?? ""
                let reason = "\(pageName)导出失败：\(error.localizedDescription)"
                self.error = reason
                message = "导出未完成"
                completion?(.failed(reason))
            }
        }
    }

    private func runQueue(_ urls: [URL], operation: String, options: [String: Any] = [:]) {
        cancelCrop()
        let enhancing = operation == "enhance"
        if !enhancing {
            for url in urls { pageProcessingErrors.removeValue(forKey: url) }
        }
        let translating = operation == "both" || operation == "translate"
        let precedingRelease = translating ? modelReleaseTask : nil
        var values = options
        let images = urls.map(\.path)
        values["images"] = images
        values["operation"] = operation
        values["super_resolution"] = srSettings
        if options["mode"] as? String == "vision", operation == "both" || operation == "translate" {
            let requested = Set(images)
            var previousImages: [String: String] = [:]
            // The chapter starts with a cover. Its image is never used as context.
            for (previous, current) in zip(files.dropFirst(), files.dropFirst(2)) where requested.contains(current.path) {
                previousImages[current.path] = previous.path
            }
            if !previousImages.isEmpty { values["previous_images"] = previousImages }
        }
        busy = true; error = nil; queueTotal = urls.count; queueDone = 0
        if enhancing { message = "正在准备整章超分…" }
        if precedingRelease != nil { message = "正在等待上一个模型释放…" }
        let request = UUID(); operationRequest = request
        task = Task { [self] in
            var startedImages = Set<String>()
            defer {
                // No newer processing task can start while this one owns busy.
                busy = false; task = nil
                if Task.isCancelled { message = "已停止；已完成的结果已保存" }
                if !shuttingDown, !enhancing, request == operationRequest || selected.map({ startedImages.contains($0.path) }) == true {
                    reloadPage()
                }
            }
            do {
                await precedingRelease?.value
                try Task.checkCancellation()
                if translating { modelReleaseNotice = nil }
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
        if cropSession != nil { cancelCrop(); return }
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
    private func prepareForShutdown() {
        shuttingDown = true
        folderRequest = nil; connectionRequest = nil
        pageTask?.cancel(); readingTask?.cancel(); task?.cancel(); modelTimer?.invalidate()
    }

    func shutdown() {
        prepareForShutdown()
        nativeCore.shutdown()
    }

    func shutdownAndWait(_ completion: @escaping @Sendable () -> Void) {
        prepareForShutdown()
        nativeCore.shutdownAndWait(completion)
    }

    func shutdownAndWait() async {
        await withCheckedContinuation { done in shutdownAndWait { done.resume() } }
    }

}

@MainActor final class KomaAppDelegate: NSObject, NSApplicationDelegate {
    var reader: Reader?
    private var terminationPending = false

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let reader else { return .terminateNow }
        if !terminationPending {
            terminationPending = true
            reader.shutdownAndWait {
                // Quit may have entered AppKit's nested modal loop while
                // handling a main-queue callback. Reply in that loop directly.
                RunLoop.main.perform(inModes: [.common, .modalPanel]) {
                    MainActor.assumeIsolated { sender.reply(toApplicationShouldTerminate: true) }
                }
            }
        }
        return .terminateLater
    }
}

#if !KOMA_REVIEW_HARNESS
@main struct KomaReaderApp: App {
    @NSApplicationDelegateAdaptor(KomaAppDelegate.self) private var appDelegate
    @StateObject private var reader = Reader()
    var body: some Scene {
        WindowGroup("Koma · 本地漫画翻译") {
            ContentView(reader: reader).tint(ReaderPalette.accent)
                .onAppear { appDelegate.reader = reader }
        }
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

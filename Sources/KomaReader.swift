import SwiftUI
import AppKit
import CryptoKit

struct TextBlock: Codable, Identifiable {
    var id: String
    var box: [Double]
    var source: String
    var translation: String
    var error: String
    var vertical: Bool
    var edited: Bool
}
struct PageResult: Codable {
    var version: Int
    var width: Int
    var height: Int
    var blocks: [TextBlock]
    var model: String
    var ocr: String
}
struct WorkerReply: Decodable { var ok: Bool; var error: String?; var cache: String }

@MainActor final class Reader: ObservableObject {
    @Published var files: [URL] = []
    @Published var selected: URL?
    @Published var page: PageResult?
    @Published var image: NSImage?
    @Published var folderName = "本地漫画"
    @Published var busy = false
    @Published var message = "打开漫画文件夹，开始阅读"
    @Published var error: String?
    @Published var selectedBlock: String?
    @Published var displayMode = 1
    @Published var zoom = 1.0
    @Published var model = "qwen3:4b-q6k"
    @Published var availableModels: [String] = ["qwen3:4b-q6k"]
    @Published var engineReady = false
    @Published var queueTotal = 0
    @Published var queueDone = 0
    private var process: Process?
    private var ollama: Process?
    private var task: Task<Void, Never>?
    private var timer: Timer?
    private var modelTimer: Timer?
    private var cacheURL: URL?
    private var statusURL: URL?
    private var started = false
    private var folderRequest = UUID()
    let root: URL
    let cacheDir = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/KomaReader/pages")

    init() {
        let configured = Bundle.main.object(forInfoDictionaryKey: "ProjectRoot") as? String
        root = URL(fileURLWithPath: configured ?? FileManager.default.currentDirectoryPath)
        if let saved = UserDefaults.standard.string(forKey: "model") { model = saved }
    }

    func startup() {
        guard !started else { return }
        started = true
        startEngine()
        modelTimer = Timer.scheduledTimer(withTimeInterval: 10, repeats: true) { [weak self] _ in
            Task { @MainActor in _ = await self?.refreshModels() }
        }
        if let folder = UserDefaults.standard.string(forKey: "folder") {
            loadFolder(URL(fileURLWithPath: folder), restore: true)
        }
    }

    func startEngine() {
        let executable = root.appendingPathComponent(".runtime/ollama/ollama")
        guard FileManager.default.isExecutableFile(atPath: executable.path) else {
            message = "翻译引擎未安装；请运行安装脚本"
            return
        }
        Task {
            if !(await refreshModels()) {
                let p = Process()
                p.executableURL = executable
                p.arguments = ["serve"]
                var env = ProcessInfo.processInfo.environment
                env["OLLAMA_HOST"] = "127.0.0.1:11439"
                env["OLLAMA_MODELS"] = root.appendingPathComponent(".models/ollama").path
                env["OLLAMA_NO_CLOUD"] = "1"
                env["OLLAMA_NUM_PARALLEL"] = "1"
                env["OLLAMA_MAX_LOADED_MODELS"] = "1"
                p.environment = env
                p.standardOutput = FileHandle.nullDevice
                p.standardError = FileHandle.nullDevice
                do { try p.run(); ollama = p } catch { self.error = error.localizedDescription }
                for _ in 0..<20 {
                    try? await Task.sleep(for: .seconds(1))
                    if await refreshModels() { break }
                }
            }
        }
    }

    func refreshModels() async -> Bool {
        do {
            var request = URLRequest(url: URL(string: "http://127.0.0.1:11439/api/tags")!)
            request.timeoutInterval = 3
            let (data, _) = try await URLSession.shared.data(for: request)
            let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
            let models = (json?["models"] as? [[String: Any]] ?? []).filter {
                $0["remote_host"] == nil && $0["remote_model"] == nil
            }.compactMap { $0["name"] as? String }.filter { !$0.lowercased().contains("cloud") }.sorted()
            availableModels = models.isEmpty ? [model] : models
            if !models.isEmpty && !models.contains(model) { model = models[0] }
            engineReady = !models.isEmpty
            return true
        } catch { engineReady = false; return false }
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
            UserDefaults.standard.set(url.path, forKey: "folder")
            let last = UserDefaults.standard.string(forKey: "lastPage")
            let initial = restore ? (images.first { $0.path == last } ?? images[0]) : images[0]
            select(initial)
        } catch { self.error = "无法打开文件夹：\(error.localizedDescription)" }
        }
    }

    func cacheFor(_ url: URL) throws -> URL {
        let data = try Data(contentsOf: url, options: .mappedIfSafe)
        let digest = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        return cacheDir.appendingPathComponent(digest + ".json")
    }

    func select(_ url: URL) {
        selected = url; selectedBlock = nil; error = nil
        image = NSImage(contentsOf: url)
        if image == nil { error = "无法解码这张图片：\(url.lastPathComponent)" }
        cacheURL = try? cacheFor(url)
        reloadPage()
        UserDefaults.standard.set(url.path, forKey: "lastPage")
        if !busy { message = page == nil ? "这页尚未处理 · 点击「识别并翻译」" : "已读取本地缓存 · \(page?.blocks.count ?? 0) 处文字" }
    }

    func reloadPage() {
        guard let cacheURL, FileManager.default.fileExists(atPath: cacheURL.path) else { page = nil; return }
        do {
            var cached = try JSONDecoder().decode(PageResult.self, from: Data(contentsOf: cacheURL))
            let placeholderURL = root.appendingPathComponent("backend/translation_placeholders.json")
            let placeholders = (try? JSONDecoder().decode([String].self, from: Data(contentsOf: placeholderURL))) ?? []
            let excluded = CharacterSet.whitespacesAndNewlines.union(.punctuationCharacters).union(.symbols)
            func normalize(_ value: String) -> String {
                String(String.UnicodeScalarView(value.lowercased().unicodeScalars.filter { !excluded.contains($0) }))
            }
            let invalid = Set(placeholders.map(normalize))
            for i in cached.blocks.indices where !cached.blocks[i].edited {
                if invalid.contains(normalize(cached.blocks[i].translation)) {
                    cached.blocks[i].error = "模型返回了占位文字，没有提供实际译文"
                }
            }
            page = cached
        }
        catch { page = nil; self.error = "缓存无法读取：\(error.localizedDescription)" }
    }

    func move(_ offset: Int) {
        guard let selected, let i = files.firstIndex(of: selected), files.indices.contains(i + offset) else { return }
        select(files[i + offset])
    }

    func saveEdit(id: String, source: String, translation: String) {
        guard !busy, var p = page, let i = p.blocks.firstIndex(where: { $0.id == id }), let cacheURL else { return }
        let sourceChanged = source != p.blocks[i].source
        p.blocks[i].source = source
        p.blocks[i].translation = sourceChanged ? "" : translation
        p.blocks[i].edited = !sourceChanged && !translation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        p.blocks[i].error = ""
        do {
            try JSONEncoder().encode(p).write(to: cacheURL, options: .atomic)
            page = p
            message = sourceChanged ? "日文已修改，旧译文已清除；点击翻译生成新译文" : "修改已保存；手改的译文会保留"
        } catch { self.error = "保存失败：\(error.localizedDescription)" }
    }

    func retryBlock(_ id: String) {
        guard !busy, var p = page, let i = p.blocks.firstIndex(where: { $0.id == id }), let cacheURL else { return }
        p.blocks[i].translation = ""; p.blocks[i].error = ""; p.blocks[i].edited = false
        do {
            try JSONEncoder().encode(p).write(to: cacheURL, options: .atomic)
            page = p; run(operation: "translate", blockID: id)
        } catch { self.error = error.localizedDescription }
    }

    func run(operation: String = "both", batch: Bool = false, blockID: String? = nil) {
        guard !busy, let selected else { return }
        let urls = batch ? files : [selected]
        busy = true; error = nil; queueTotal = urls.count; queueDone = 0
        UserDefaults.standard.set(model, forKey: "model")
        task = Task {
            var failures = 0
            for url in urls {
                if Task.isCancelled { break }
                message = "准备处理 \(url.lastPathComponent)…"
                do {
                    try await processPage(url, operation: operation, blockID: blockID)
                    if self.selected == url { reloadPage() }
                    let cached = try JSONDecoder().decode(PageResult.self, from: Data(contentsOf: cacheFor(url)))
                    failures += cached.blocks.filter { !$0.error.isEmpty }.count
                } catch {
                    if !Task.isCancelled { self.error = error.localizedDescription; failures += 1 }
                    if self.selected == url { reloadPage() }
                }
                queueDone += 1
            }
            timer?.invalidate(); timer = nil; process = nil; busy = false
            if Task.isCancelled { message = "已停止；已完成的结果已保存" }
            else if failures > 0 { message = "处理结束，有 \(failures) 处问题；请查看提示后重试" }
            else { message = "处理完成 · \(queueDone) 页 · 结果已保存到本机" }
            task = nil
        }
    }

    func processPage(_ url: URL, operation: String, blockID: String? = nil) async throws {
        let status = FileManager.default.temporaryDirectory.appendingPathComponent("koma-\(UUID().uuidString).json")
        statusURL = status
        let p = Process()
        p.executableURL = root.appendingPathComponent(".venv/bin/python")
        p.arguments = [root.appendingPathComponent("backend/worker.py").path, "--image", url.path,
                       "--operation", operation, "--model", model, "--status", status.path]
        if let blockID { p.arguments?.append(contentsOf: ["--block-id", blockID]) }
        p.currentDirectoryURL = root
        let out = Pipe()
        p.standardOutput = out
        // Logs stay on disk; never fill a stderr pipe while running a model.
        let log = root.appendingPathComponent(".runtime/ocr.log")
        FileManager.default.createFile(atPath: log.path, contents: nil)
        let logHandle = try FileHandle(forWritingTo: log)
        p.standardError = logHandle
        process = p
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in
            Task { @MainActor in
                if let data = try? Data(contentsOf: status),
                   let obj = try? JSONSerialization.jsonObject(with: data) as? [String: String],
                   let text = obj["message"] { self?.message = text }
            }
        }
        defer { timer?.invalidate(); timer = nil; try? logHandle.close(); try? FileManager.default.removeItem(at: status) }
        let data: Data = try await withCheckedThrowingContinuation { continuation in
            p.terminationHandler = { _ in
                continuation.resume(returning: out.fileHandleForReading.readDataToEndOfFile())
            }
            do { try p.run() } catch { p.terminationHandler = nil; continuation.resume(throwing: error) }
        }
        if Task.isCancelled { throw CancellationError() }
        guard let reply = try? JSONDecoder().decode(WorkerReply.self, from: data) else {
            throw NSError(domain: "Koma", code: 1, userInfo: [NSLocalizedDescriptionKey: "处理进程意外结束。详细信息在 .runtime/ocr.log；已完成的缓存仍保留。"])
        }
        if !reply.ok {
            throw NSError(domain: "Koma", code: 2, userInfo: [NSLocalizedDescriptionKey: reply.error ?? "处理失败"])
        }
    }

    func cancel() { task?.cancel(); if process?.isRunning == true { process?.terminate() } }
    func shutdown() { cancel(); modelTimer?.invalidate(); if ollama?.isRunning == true { ollama?.terminate() } }
}

struct ContentView: View {
    @ObservedObject var reader: Reader
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
                        ScrollView([.horizontal, .vertical]) {
                            let ratio = image.size.height / max(image.size.width, 1)
                            let w = max(300, geo.size.width - 48) * reader.zoom
                            ZStack(alignment: .topLeading) {
                                Image(nsImage: image).resizable().frame(width: w, height: w * ratio)
                                if reader.displayMode != 0, let page = reader.page {
                                    ForEach(page.blocks) { block in
                                        region(block, page: page, width: w, height: w * ratio)
                                    }
                                }
                            }.frame(width: w, height: w * ratio).padding(24)
                        }
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
                if reader.busy { ProgressView().controlSize(.small) }
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
                if reader.busy {
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
        .task { reader.startup() }
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
            if reader.displayMode == 2 && !block.translation.isEmpty && block.error.isEmpty {
                RoundedRectangle(cornerRadius: 3).fill(.white)
                Text(block.translation).font(.system(size: 15 * reader.zoom)).foregroundStyle(.black)
                    .minimumScaleFactor(0.4).multilineTextAlignment(.center)
                    .frame(width: max(8, w - 6), height: max(8, h - 6)).padding(3)
            } else { RoundedRectangle(cornerRadius: 3).fill(.teal.opacity(active ? 0.18 : 0.05)) }
            RoundedRectangle(cornerRadius: 3).strokeBorder(block.error.isEmpty ? Color.teal : Color.orange, lineWidth: active ? 3 : 1)
            if reader.displayMode == 1 || !block.error.isEmpty {
                Text(block.id).font(.system(size: 10, weight: .bold)).foregroundStyle(.white).padding(.horizontal, 4).padding(.vertical, 2).background(.teal, in: RoundedRectangle(cornerRadius: 3)).offset(y: -16)
            }
        }.frame(width: w, height: h).offset(x: x, y: y)
            .contentShape(Rectangle()).onTapGesture { reader.selectedBlock = block.id }
            .help(block.translation.isEmpty ? block.source : block.translation)
    }

    var inspector: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("对白对照").font(.title3.weight(.semibold))
            Picker("本地模型", selection: $reader.model) {
                ForEach(reader.availableModels, id: \.self) { Text($0).tag($0) }
            }.disabled(reader.busy)
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
                                            disabled: reader.busy, save: { source, target in
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
    let save: (String, String) -> Void
    let retry: () -> Void
    let select: () -> Void
    @State private var source = ""
    @State private var target = ""
    var changed: Bool { source != block.source || target != block.translation }
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("\(block.id)").font(.caption.bold()).foregroundStyle(.teal)
                Text(block.vertical ? "竖排" : "横排").font(.caption2).foregroundStyle(.secondary)
                Spacer()
                if block.edited { Text("已手动修改").font(.caption2).foregroundStyle(.teal) }
            }
            TextField("日文原文", text: $source, axis: .vertical).textFieldStyle(.plain).font(.system(size: 13)).lineLimit(2...8)
            Divider()
            TextField("尚未翻译", text: $target, axis: .vertical).textFieldStyle(.plain).font(.system(size: 14)).lineLimit(2...10)
            if !block.error.isEmpty { Text(block.error).font(.caption).foregroundStyle(.orange).textSelection(.enabled) }
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

@main struct KomaReaderApp: App {
    @StateObject private var reader = Reader()
    var body: some Scene {
        WindowGroup("Koma · 本地漫画翻译") { ContentView(reader: reader).tint(.teal) }
            .defaultSize(width: 1280, height: 850)
            .commands {
                CommandGroup(replacing: .newItem) {
                    Button("打开漫画文件夹…") { reader.chooseFolder() }.keyboardShortcut("o").disabled(reader.busy)
                }
            }
    }
}

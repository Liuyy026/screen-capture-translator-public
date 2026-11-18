import SwiftUI
import AppKit

private typealias StatusState<Value> = SwiftUI.State<Value>

@MainActor struct ContentView: View {
    @ObservedObject var reader: Reader
    let startsServices: Bool
    @StateObject private var workspace: ReaderWorkspaceState
    @StateObject private var imagePan = ImagePanController()

    init(reader: Reader, startsServices: Bool = true, workspace: ReaderWorkspaceState? = nil) {
        self.reader = reader
        self.startsServices = startsServices
        _workspace = StateObject(wrappedValue: workspace ?? ReaderWorkspaceState())
    }

    private var working: Bool { reader.busy || reader.preparingImage }
    private var pageNumber: String {
        guard let selected = reader.selected, let index = reader.files.firstIndex(of: selected) else { return "0 / 0" }
        return "\(index + 1) / \(reader.files.count)"
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                VStack(spacing: 0) {
                    if let image = reader.image {
                        ReaderCanvasView(reader: reader, controller: imagePan, image: image,
                                         showsSelection: workspace.showsInspector, select: selectBlock)
                    } else {
                        welcome.frame(maxWidth: .infinity, maxHeight: .infinity)
                            .background(ReaderPalette.canvas)
                    }
                    Divider().overlay(ReaderPalette.separator)
                    readingControls
                }.frame(maxWidth: .infinity, maxHeight: .infinity)
                if workspace.showsInspector {
                    Divider().overlay(ReaderPalette.separator)
                    ReaderInspectorView(reader: reader, workspace: workspace)
                }
            }.frame(maxWidth: .infinity, maxHeight: .infinity)
            ReaderStatusView(reader: reader, showDialogue: selectBlock)
        }
        .foregroundStyle(ReaderPalette.text).background(ReaderPalette.bar).tint(ReaderPalette.accent)
        .navigationTitle(reader.folderName)
        .navigationSubtitle(reader.selected?.lastPathComponent ?? "Koma · 本地漫画翻译")
        .toolbar { readerToolbar }
        .frame(minWidth: 840, minHeight: 660)
        .sheet(isPresented: $reader.showConnectionSettings) { TranslationSettingsView(reader: reader).tint(ReaderPalette.accent) }
        .sheet(isPresented: $workspace.showsExportOptions) { ReaderExportOptionsView(reader: reader) }
        .task { if startsServices { reader.startup() } }
        .onReceive(reader.$page) { page in
            guard let page else { return }
            if let cache = reader.editCacheIdentity { workspace.reconcile(cache: cache, page: page) }
            if !page.blocks.contains(where: { $0.id == reader.selectedBlock }) {
                reader.selectedBlock = page.blocks.first?.id
            }
        }
        .onChange(of: reader.selected) { _, _ in workspace.showsPages = false }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.willTerminateNotification)) { _ in reader.shutdown() }
    }

    @ToolbarContentBuilder private var readerToolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .navigation) {
            Button { reader.chooseFolder() } label: { Label("打开文件夹", systemImage: "folder") }
                .disabled(reader.busy).accessibilityIdentifier("open-folder")
        }
        ToolbarItemGroup(placement: .primaryAction) {
            Button { workspace.showsProcessingSettings.toggle() } label: {
                Label("处理设置", systemImage: "slider.horizontal.3")
            }.disabled(reader.busy).accessibilityIdentifier("processing-settings")
                .popover(isPresented: $workspace.showsProcessingSettings) {
                    ReaderProcessingSettingsView(reader: reader) {
                        workspace.showsProcessingSettings = false
                        reader.showConnectionSettings = true
                    }
                }
            Button { reader.enhanceChapter() } label: {
                Label("整章超分", systemImage: "viewfinder").labelStyle(.titleAndIcon)
            }.disabled(!reader.canEnhanceChapter).accessibilityIdentifier("enhance-chapter")
                .help("按处理设置超分整个文件夹；前置增强关闭时使用 MetalFX。")
            Button { workspace.showsExportOptions = true } label: {
                Label("导出漫画", systemImage: "square.and.arrow.up").labelStyle(.titleAndIcon)
            }.disabled(!reader.canEnhanceChapter).accessibilityIdentifier("export-chapter")
                .help("选择要导出的页面、文件夹名称和保存位置。")
        }
        translationToolbarItem
        ToolbarItem(placement: .primaryAction) {
            Menu {
                Button("只识别本页日文") { reader.run(operation: "ocr") }
                Button("翻译整个文件夹") { reader.run(batch: true) }.disabled(!reader.engineReady)
            } label: { Image(systemName: "ellipsis") }
                .disabled(reader.selected == nil || working || reader.openingFolder)
                .help("更多处理操作").accessibilityIdentifier("more-processing")
        }
    }

    @ToolbarContentBuilder private var translationToolbarItem: some ToolbarContent {
        if #available(macOS 26.0, *) {
            // The button owns its capsule; a second shared capsule clips its edges.
            ToolbarItem(placement: .primaryAction) { translationButton }
                .sharedBackgroundVisibility(.hidden)
        } else {
            ToolbarItem(placement: .primaryAction) { translationButton }
        }
    }

    private var translationButton: some View {
        Group {
            if working {
                Button { reader.cancel() } label: {
                    Label(reader.exportingChapter ? "停止导出" : "停止处理", systemImage: "stop.fill").frame(width: 102)
                }.accessibilityIdentifier("stop-processing")
            } else {
                Button { reader.run() } label: {
                    Label("识别并翻译", systemImage: "character.bubble").frame(width: 102)
                }.disabled(reader.selected == nil || !reader.engineReady || reader.openingFolder)
                    .accessibilityIdentifier("translate-page")
            }
        }.labelStyle(.titleAndIcon).buttonStyle(ReaderToolbarPrimaryButtonStyle())
    }

    private var readingControls: some View {
        HStack(spacing: 8) {
            Picker("显示", selection: $reader.displayMode) {
                Text(reader.srEngine == "off" ? "原图" : "增强原文").tag(0)
                Text("文字框").tag(1)
                Text("中文").tag(2)
            }.pickerStyle(.segmented).labelsHidden().frame(width: 186)
                .accessibilityIdentifier("display-mode")
            Spacer(minLength: 8)
            HStack(spacing: 6) {
                Button { reader.move(-1) } label: { Image(systemName: "chevron.left") }
                    .disabled(reader.selected == reader.files.first || reader.files.isEmpty)
                    .help("上一页").accessibilityIdentifier("previous-page")
                Button { workspace.showsPages.toggle() } label: {
                    HStack(spacing: 6) {
                        Text(pageNumber).monospacedDigit()
                        Image(systemName: "chevron.up").font(.system(size: 9, weight: .semibold))
                    }.frame(minWidth: 58, minHeight: 22)
                }.buttonStyle(ReaderControlButtonStyle(selected: workspace.showsPages))
                    .disabled(reader.files.isEmpty).accessibilityIdentifier("page-picker")
                    .help("展开章节缩略图")
                    .popover(isPresented: $workspace.showsPages, arrowEdge: .bottom) {
                        ReaderPagePicker(reader: reader) { url in
                            reader.select(url); workspace.showsPages = false
                        }
                    }
                Button { reader.move(1) } label: { Image(systemName: "chevron.right") }
                    .disabled(reader.selected == reader.files.last || reader.files.isEmpty)
                    .help("下一页").accessibilityIdentifier("next-page")
            }.buttonStyle(ReaderControlButtonStyle()).font(.system(size: 12))
            Spacer(minLength: 8)
            Picker("画面缩放", selection: $reader.zoom) {
                ForEach(Array(stride(from: 0.5, through: 3.0, by: 0.25)), id: \.self) { zoom in
                    Text(zoom == 1 ? "适合窗口" : "\(Int(zoom * 100))%").tag(zoom)
                }
            }.labelsHidden().frame(width: 102).disabled(reader.image == nil)
                .accessibilityIdentifier("reader-zoom")
            Button { workspace.showsInspector.toggle() } label: {
                Label("对白", systemImage: "sidebar.right").font(.system(size: 12))
                    .padding(.horizontal, 7).padding(.vertical, 5)
                    .background(workspace.showsInspector ? ReaderPalette.selection : .clear,
                                in: RoundedRectangle(cornerRadius: 6))
            }.buttonStyle(.plain).accessibilityIdentifier("toggle-inspector")
                .help(workspace.showsInspector ? "收起对白对照" : "展开对白对照")
        }.padding(.horizontal, 12).padding(.vertical, 8).controlSize(.small)
            .background(ReaderPalette.bar)
    }

    private var welcome: some View {
        VStack(spacing: 18) {
            Image(systemName: "text.viewfinder").font(.system(size: 48, weight: .ultraLight))
                .foregroundStyle(ReaderPalette.accent)
            Text("打开漫画，开始阅读").font(.system(size: 24, weight: .semibold))
            Text("支持 WebP、PNG、JPG。\n识别日文，翻译并回填到漫画页。")
                .multilineTextAlignment(.center).foregroundStyle(ReaderPalette.secondary)
            Button("打开漫画文件夹") { reader.chooseFolder() }.buttonStyle(ReaderPrimaryButtonStyle())
            Button("先试试内置测试页") { reader.loadFolder(reader.root.appendingPathComponent("samples")) }
                .buttonStyle(.link)
        }.padding(24)
    }

    private func selectBlock(_ id: String?) {
        guard let id else { return }
        reader.selectedBlock = id
        workspace.showsInspector = true
    }
}

private struct ReaderCanvasView: View {
    @ObservedObject var reader: Reader
    @ObservedObject var controller: ImagePanController
    let image: NSImage
    let showsSelection: Bool
    let select: (String?) -> Void

    var body: some View {
        GeometryReader { geometry in
            let layout = ReaderCanvasGeometry.layout(image: image.size, viewport: geometry.size, zoom: reader.zoom)
            PannableImageScrollView(controller: controller, documentSize: layout.document, pageID: reader.selected) {
                ZStack(alignment: .topLeading) {
                    imageWithOverlays(size: layout.image)
                        .offset(x: layout.inset.x, y: layout.inset.y)
                }.frame(width: layout.document.width, height: layout.document.height, alignment: .topLeading)
                    .contentShape(Rectangle())
                    .help("放大后按住鼠标左键拖动图片；单击文字框定位对白。")
            }.onChange(of: reader.zoom) { _, _ in controller.endDrag() }
        }.background(ReaderPalette.canvas).accessibilityElement(children: .contain)
            .accessibilityIdentifier("reader-canvas")
    }

    private func imageWithOverlays(size: CGSize) -> some View {
        ZStack(alignment: .topLeading) {
            Image(nsImage: image).resizable().frame(width: size.width, height: size.height)
            if reader.displayMode != 0, let page = reader.page, page.width > 0, page.height > 0 {
                if reader.displayMode == 2 {
                    ForEach(reader.readingPresentations) { item in
                        ReadingOverlayView(presentation: item, scale: size.width / Double(page.width),
                                           verticalScale: size.height / Double(page.height),
                                           selected: showsSelection && item.region.block_ids.contains(reader.selectedBlock ?? ""),
                                           selectionColor: ReaderPalette.marker) {
                            select(item.region.block_ids.first)
                        }
                    }
                }
                ForEach(page.blocks) { block in
                    let marked = !block.keepsOriginal && (reader.displayMode == 1 || !block.error.isEmpty)
                    if (marked || block.keepsOriginal), block.box.count == 4,
                       block.box.allSatisfy(\.isFinite) {
                        region(block, page: page, size: size, marked: marked)
                    }
                }
            }
        }.frame(width: size.width, height: size.height)
    }

    private func region(_ block: TextBlock, page: PageResult, size: CGSize, marked: Bool) -> some View {
        let x = block.box[0] / Double(page.width) * size.width
        let y = block.box[1] / Double(page.height) * size.height
        let width = max(12, (block.box[2] - block.box[0]) / Double(page.width) * size.width)
        let height = max(12, (block.box[3] - block.box[1]) / Double(page.height) * size.height)
        let active = showsSelection && reader.selectedBlock == block.id
        return ZStack(alignment: .topLeading) {
            Color.clear
            if marked {
                RoundedRectangle(cornerRadius: 3).fill(ReaderPalette.marker.opacity(active ? 0.12 : 0.035))
                RoundedRectangle(cornerRadius: 3).strokeBorder(block.error.isEmpty ? ReaderPalette.marker : Color.orange,
                                                             lineWidth: active ? 2 : 1)
                Text(block.id).font(.system(size: 10, weight: .semibold)).foregroundStyle(.white)
                    .padding(.horizontal, 4).padding(.vertical, 2)
                    .background(ReaderPalette.marker, in: RoundedRectangle(cornerRadius: 3)).offset(y: -16)
            }
        }.frame(width: width, height: height).contentShape(Rectangle())
            .onTapGesture { select(block.id) }
            .help(block.needsOCRReview ? (block.ocr_review?.reason ?? "疑似非文字，请核对原图") : (block.keepsOriginal || block.translation.isEmpty ? block.source : block.translation))
            .accessibilityLabel("定位对白 " + block.id).accessibilityIdentifier("comic-block-" + block.id)
            .offset(x: x, y: y)
    }
}

private struct ReaderStatusView: View {
    @ObservedObject var reader: Reader
    let showDialogue: (String) -> Void
    @StatusState private var showsDiagnostics = false

    private var layoutNotice: String? {
        guard reader.displayMode == 2, reader.page != nil else { return nil }
        if let error = reader.readingLayoutError { return error }
        if reader.readingRegions == nil { return "正在整理中文排版…" }
        var notes: [String] = []
        if reader.unreadableOverlayCount > 0 { notes.append("\(reader.unreadableOverlayCount) 处译文暂未覆盖到图片，可在对白栏阅读。") }
        let review = Set((reader.page?.blocks ?? []).filter(\.needsOCRReview).map(\.id))
        if !review.isEmpty { notes.append("\(review.count) 处疑似非文字已暂停处理，可在「本页诊断」核对。") }
        let markers = reader.readingPreserved.keys.filter { !review.contains($0) }.count
        if markers > 0 { notes.append("\(markers) 处疑似页码已保留原图，请核对原文。") }
        return notes.isEmpty ? nil : notes.joined(separator: " ")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Divider().overlay(ReaderPalette.separator)
            if let error = reader.error ?? reader.enhancementWarning {
                HStack(alignment: .top, spacing: 8) {
                    Image(systemName: "exclamationmark.triangle").foregroundStyle(.orange)
                    Text(error).textSelection(.enabled).lineLimit(3).help(error)
                }.font(.caption).padding(.horizontal, 12).padding(.top, 8)
                    .accessibilityIdentifier("reader-error")
            }
            if let notice = layoutNotice {
                Text(notice).font(.caption).foregroundStyle(ReaderPalette.secondary)
                    .padding(.horizontal, 12).padding(.top, 6).lineLimit(2).help(notice)
            }
            HStack(spacing: 8) {
                if reader.busy || reader.loadingPage { ProgressView().controlSize(.mini) }
                Text(reader.message).lineLimit(2).help(reader.message)
                if reader.busy && reader.queueTotal > 1 { Text("\(reader.queueDone) / \(reader.queueTotal)").monospacedDigit() }
                if let export = reader.lastChapterExport {
                    Button("在访达显示") { NSWorkspace.shared.activateFileViewerSelecting([export]) }
                        .buttonStyle(.link).help(export.path)
                }
                Spacer(minLength: 8)
                Button("本页诊断") { showsDiagnostics.toggle() }
                    .buttonStyle(.link).disabled(reader.selected == nil)
                    .accessibilityIdentifier("page-diagnostics")
                    .popover(isPresented: $showsDiagnostics, arrowEdge: .bottom) {
                        ReaderDiagnosticsView(reader: reader) { id in
                            showsDiagnostics = false
                            showDialogue(id)
                        }
                    }
                Image(systemName: reader.engineReady ? "checkmark.circle" : "exclamationmark.circle")
                Text(reader.connection.name + (reader.engineReady ? " 已连接" : " 未连接"))
            }.font(.system(size: 11)).foregroundStyle(ReaderPalette.secondary)
                .padding(.horizontal, 12).padding(.vertical, 7)
        }.background(ReaderPalette.bar)
    }
}

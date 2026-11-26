import SwiftUI
import AppKit

private typealias ViewState<Value> = SwiftUI.State<Value>

@MainActor struct ContentView: View {
    @ObservedObject var reader: Reader
    let startsServices: Bool
    @StateObject private var workspace: ReaderWorkspaceState
    @StateObject private var imagePan = ImagePanController()
    @AppStorage("reader.inspectorWidth") private var preferredInspectorWidth = 356.0
    @ViewState private var inspectorDrag: InspectorDrag?
    @ViewState private var showsChapterTypography = false

    private struct InspectorDrag {
        let initialWidth: CGFloat
        var translation: CGFloat = 0
    }

    private static let inspectorDividerWidth: CGFloat = 9

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
            GeometryReader { geometry in
                readerSplit(windowWidth: geometry.size.width)
            }
            ReaderStatusView(reader: reader, showDialogue: selectBlock)
        }
        .foregroundStyle(ReaderPalette.text).background(ReaderPalette.bar).tint(ReaderPalette.accent)
        .navigationTitle(reader.folderName)
        .navigationSubtitle(reader.selected?.lastPathComponent ?? "Koma · 本地漫画翻译")
        .toolbar { readerToolbar }
        .frame(minWidth: 840, minHeight: 660)
        .sheet(isPresented: $reader.showConnectionSettings) { TranslationSettingsView(reader: reader).tint(ReaderPalette.accent) }
        .sheet(isPresented: $workspace.showsExportOptions) { ReaderExportOptionsView(reader: reader) }
        .sheet(isPresented: $showsChapterTypography) { ReaderChapterTypographyEditor(reader: reader) }
        .sheet(isPresented: Binding(get: {
            reader.cropSession.map { $0.phase != .selecting } ?? false
        }, set: { if !$0, reader.cropSession?.phase == .reviewing { reader.cancelCrop() } })) {
            ReaderCropReviewView(reader: reader, workspace: workspace)
        }
        .task { if startsServices { reader.startup() } }
        .onReceive(reader.$page) { page in
            guard let page else { return }
            if let cache = reader.editCacheIdentity { workspace.reconcile(cache: cache, page: page) }
        }
        .onChange(of: reader.selected) { _, _ in workspace.showsPages = false }
        .onChange(of: workspace.showsInspector) { _, isShown in
            if !isShown { inspectorDrag = nil }
        }
    }

    private func clampedInspectorWidth(_ width: CGFloat, windowWidth: CGFloat) -> CGFloat {
        let minimum: CGFloat = 250
        // Leave space for the normal reading controls even at the minimum window size.
        let maximum = max(minimum, min(580, windowWidth - 560 - Self.inspectorDividerWidth))
        return min(maximum, max(minimum, width))
    }

    private func readerSplit(windowWidth: CGFloat) -> some View {
        let proposedWidth = inspectorDrag.map { $0.initialWidth - $0.translation } ?? CGFloat(preferredInspectorWidth)
        let width = clampedInspectorWidth(proposedWidth, windowWidth: windowWidth)
        return HStack(spacing: 0) {
            VStack(spacing: 0) {
                if let image = reader.image {
                    ReaderCanvasView(reader: reader, controller: imagePan, image: reader.cropSession?.original ?? image,
                                     showsSelection: workspace.showsInspector, select: selectBlock)
                } else {
                    welcome.frame(maxWidth: .infinity, maxHeight: .infinity)
                        .background(ReaderPalette.canvas)
                }
                Divider().overlay(ReaderPalette.separator)
                if reader.cropSession?.phase == .selecting {
                    HStack {
                        Text("在原图上拖动，框住一列或一行漏掉的日文")
                        Spacer()
                        Button("取消圈选", action: reader.cancelCrop).keyboardShortcut(.cancelAction)
                    }.font(.callout).padding(.horizontal, 12).padding(.vertical, 5)
                }
                readingControls
            }.frame(maxWidth: .infinity, maxHeight: .infinity)
            if workspace.showsInspector {
                Rectangle().fill(inspectorDrag == nil ? ReaderPalette.separator : ReaderPalette.accent)
                    .frame(width: 1)
                    .frame(width: Self.inspectorDividerWidth)
                    .frame(maxHeight: .infinity)
                    .overlay {
                        ReaderInspectorDivider { drag in
                            switch drag {
                            case .began:
                                inspectorDrag = InspectorDrag(initialWidth: width)
                            case .changed(let translation):
                                inspectorDrag?.translation = translation
                            case .ended(let translation):
                                if let inspectorDrag, translation != 0 {
                                    let finalWidth = clampedInspectorWidth(inspectorDrag.initialWidth - translation,
                                                                             windowWidth: windowWidth)
                                    if finalWidth != inspectorDrag.initialWidth {
                                        preferredInspectorWidth = Double(finalWidth)
                                    }
                                }
                                inspectorDrag = nil
                            }
                        }
                    }
                ReaderInspectorView(reader: reader, workspace: workspace)
                    .frame(width: width)
            }
        }.frame(maxWidth: .infinity, maxHeight: .infinity)
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
                Button("圈选补识别") { workspace.showsInspector = true; reader.beginCrop() }.disabled(!reader.canBeginCrop)
                Button("只识别本页日文") { reader.run(operation: "ocr") }
                Button("翻译整个文件夹") { reader.run(batch: true) }.disabled(!reader.engineReady)
                Button("章节排版默认…") { showsChapterTypography = true }
                    .disabled(reader.selected == nil)
                    .accessibilityIdentifier("chapter-typography")
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
                    .contentShape(Rectangle())
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
            Button("先试试内置测试页") { reader.loadFolder(reader.samplesDirectory) }
                .buttonStyle(.link)
        }.padding(24)
    }

    private func selectBlock(_ id: String) {
        reader.selectedBlock = id
        workspace.showsInspector = true
    }
}

private struct ReaderCanvasView: View {
    @ObservedObject var reader: Reader
    @ObservedObject var controller: ImagePanController
    let image: NSImage
    let showsSelection: Bool
    let select: (String) -> Void
    @ViewState private var framePreview: FramePreview?

    private struct FramePreview {
        let identity: String
        let blockID: String
        let frame: CGRect
        let presentation: ReadingPresentation?
    }

    var body: some View {
        GeometryReader { geometry in
            let layout = ReaderCanvasGeometry.layout(image: image.size, viewport: geometry.size, zoom: reader.zoom)
            PannableImageScrollView(controller: controller, documentSize: layout.document, pageID: reader.selected,
                                    isPanEnabled: reader.cropSession == nil) {
                ZStack(alignment: .topLeading) {
                    Color.clear.contentShape(Rectangle())
                        .onTapGesture(count: 2, coordinateSpace: .local) { point in
                            clearSelection(at: point, layout: layout)
                        }
                    imageWithOverlays(size: layout.image)
                        .overlay {
                            if let session = reader.cropSession, session.phase == .selecting {
                                ReaderCropSelectionView(session: session, select: reader.previewCrop)
                            }
                        }
                        .offset(x: layout.inset.x, y: layout.inset.y)
                }.frame(width: layout.document.width, height: layout.document.height, alignment: .topLeading)
                    .contentShape(Rectangle())
                    .help("放大后按住鼠标左键拖动图片；单击文字框定位对白，双击空白处取消选中。")
            }.onChange(of: reader.zoom) { _, _ in controller.endDrag(); framePreview = nil }
                .onChange(of: reader.frameEditIdentity) { _, _ in framePreview = nil }
                .onChange(of: reader.displayMode) { _, _ in framePreview = nil }
                .onChange(of: reader.cropSession?.id) { _, _ in framePreview = nil }
        }.background(ReaderPalette.canvas).accessibilityElement(children: .contain)
            .accessibilityIdentifier("reader-canvas")
    }

    private func clearSelection(at point: CGPoint, layout: ReaderCanvasGeometry.Layout) {
        let imagePoint = CGPoint(x: point.x - layout.inset.x, y: point.y - layout.inset.y)
        if CGRect(origin: .zero, size: layout.image).contains(imagePoint), let page = reader.page,
           page.blocks.contains(where: {
               ReaderCanvasGeometry.blockFrame($0, page: page, size: layout.image)?.contains(imagePoint) == true
           }) { return }
        reader.clearBlockSelection()
    }

    private func imageWithOverlays(size: CGSize) -> some View {
        ZStack(alignment: .topLeading) {
            Image(nsImage: image).resizable().frame(width: size.width, height: size.height)
                .allowsHitTesting(false)
            if reader.cropSession == nil, reader.displayMode != 0, let page = reader.page, page.width > 0, page.height > 0 {
                if reader.displayMode == 2 {
                    ReadingPageOverlays(items: displayedPresentations, scale: size.width / Double(page.width),
                                        verticalScale: size.height / Double(page.height),
                                        selectRegion: { item in
                                            if let id = item.region.block_ids.first { select(id) }
                                        })
                }
                let markers = ReaderBlockMarker.layout(page: page, size: size, boxMode: reader.displayMode == 1)
                ForEach(markers) { marker in region(marker) }
                ForEach(markers) { marker in
                    if let badge = marker.badge {
                        Text(verbatim: String(marker.number))
                            .font(Font(ReaderBlockMarker.font))
                            .lineLimit(1).fixedSize()
                            .foregroundStyle(.white)
                            .frame(width: badge.width, height: badge.height)
                            .background(ReaderPalette.marker, in: RoundedRectangle(cornerRadius: 3))
                            .offset(x: badge.minX, y: badge.minY)
                    }
                }.allowsHitTesting(false).accessibilityHidden(true)
                if reader.displayMode == 2, let identity = reader.frameEditIdentity, let regions = reader.readingRegions {
                    textEditor(page: page, regions: regions, identity: identity, size: size)
                }
            }
        }.frame(width: size.width, height: size.height)
    }

    private var displayedPresentations: [ReadingPresentation] {
        var items = reader.readingPresentations
        guard let preview = framePreview, preview.identity == reader.frameEditIdentity, let item = preview.presentation else { return items }
        if let index = items.firstIndex(where: { $0.id == item.id }) { items[index] = item }
        else { items.append(item) }
        return items
    }

    private func textEditor(page: PageResult, regions: [ReadingRegion], identity: String, size: CGSize) -> some View {
        let pageSize = CGSize(width: page.width, height: page.height)
        let bounds = CGRect(origin: .zero, size: pageSize)
        let targets = page.blocks.compactMap { block -> ReaderTextFrameEditor.Target? in
            guard block.needsReadingOverlay, let region = regions.first(where: { $0.block_ids.contains(block.id) }),
                  let rect = reader.editingFrame(for: block, in: region, page: page) else { return nil }
            let frame = rect.intersection(bounds)
            return frame.isEmpty ? nil : ReaderTextFrameEditor.Target(id: block.id, frame: frame)
        }
        return ReaderTextFrameEditor(targets: targets, pageSize: pageSize,
            identity: identity + "/\(size.width)x\(size.height)", selectedID: reader.selectedBlock,
            editable: showsSelection && !reader.busy && !reader.loadingPage && !reader.openingFolder,
            invalidPreview: framePreview != nil && framePreview?.presentation == nil,
            select: { select($0) }, update: { id, frame, commit in
                guard reader.frameEditIdentity == identity, !reader.busy, reader.cropSession == nil,
                      let block = page.blocks.first(where: { $0.id == id }),
                      let region = regions.first(where: { $0.block_ids.contains(id) }) else { framePreview = nil; return }
                let preview: FramePreview
                if let existing = framePreview, existing.blockID == id, existing.frame == frame, existing.identity == identity {
                    preview = existing
                } else {
                    preview = FramePreview(identity: identity, blockID: id, frame: frame,
                                           presentation: reader.previewFrame(frame, for: block, in: region))
                }
                if commit {
                    framePreview = nil
                    if preview.presentation != nil { reader.setTextFrame(frame, for: block) }
                    else if region.block_ids.count > 1, !reader.readingPresentations.contains(where: { $0.id == region.id }),
                            reader.canStageFrame(frame, for: block, sharing: region) {
                        if reader.setTextFrame(frame, for: block) {
                            reader.message = "已保存这条对白的位置。调整同框其余对白后可一起回填。"
                        }
                    } else { reader.message = "排字框未保存。请拉宽框、缩小字号或检查同框对白。" }
                } else { framePreview = preview }
            }, cancel: { framePreview = nil })
            .frame(width: size.width, height: size.height)
            .accessibilityRepresentation {
                ForEach(targets, id: \.id) { target in
                    if let index = page.blocks.firstIndex(where: { $0.id == target.id }) {
                        Button("对白 \(index + 1)，\(page.blocks[index].translation)") { select(target.id) }
                            .accessibilityIdentifier("comic-translation-" + target.id)
                    }
                }
            }
    }

    private func region(_ marker: ReaderBlockMarker) -> some View {
        let block = marker.block
        let active = showsSelection && reader.selectedBlock == block.id
        let summary = block.needsOCRReview ? (block.ocr_review?.reason ?? "疑似非文字，请核对原图") : (block.keepsOriginal || block.translation.isEmpty ? block.source : block.translation)
        return ZStack(alignment: .topLeading) {
            Color.clear
            if marker.badge != nil {
                RoundedRectangle(cornerRadius: 3).fill(ReaderPalette.marker.opacity(active ? 0.12 : 0.035))
                RoundedRectangle(cornerRadius: 3).strokeBorder(block.error.isEmpty ? ReaderPalette.marker : Color.orange,
                                                             lineWidth: active ? 2 : 1)
            }
        }.frame(width: marker.frame.width, height: marker.frame.height).contentShape(Rectangle())
            .onTapGesture { select(block.id) }
            .help([summary, "文字框编号 " + block.id].filter { !$0.isEmpty }.joined(separator: "\n"))
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("定位对白 \(marker.number)").accessibilityIdentifier("comic-block-" + block.id)
            .accessibilityAddTraits(.isButton).accessibilityAction { select(block.id) }
            .offset(x: marker.frame.minX, y: marker.frame.minY)
    }
}

private struct ReaderStatusView: View {
    @ObservedObject var reader: Reader
    let showDialogue: (String) -> Void
    @ViewState private var showsDiagnostics = false
    @ViewState private var showsExportOmissions = false

    private var layoutNotice: String? {
        guard reader.displayMode == 2, reader.page != nil else { return nil }
        if let error = reader.readingLayoutError { return error }
        if reader.readingRegions == nil { return "正在整理中文排版…" }
        var notes: [String] = []
        if reader.unreadableOverlayCount > 0 { notes.append("\(reader.unreadableOverlayCount) 处译文暂未覆盖到图片，可在对白栏阅读。") }
        let review = Set((reader.page?.blocks ?? []).filter(\.needsOCRReview).map(\.id))
        if !review.isEmpty { notes.append("\(review.count) 处疑似非文字已暂停处理，可在「本页诊断」核对。") }
        let pending = Set((reader.page?.blocks ?? []).filter { !$0.keepsOriginal && $0.source_translation_pending == true }.map(\.id))
        if !pending.isEmpty { notes.append("\(pending.count) 处已补充日文，等待核对或重新翻译。") }
        let markers = reader.readingPreserved.keys.filter { !review.contains($0) && !pending.contains($0) }.count
        if markers > 0 { notes.append("\(markers) 处已保留原图，可在对白栏查看原因。") }
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
                    if !reader.lastChapterExportOmissions.isEmpty {
                        Button("导出未回填详情") { showsExportOmissions.toggle() }
                            .buttonStyle(.link)
                            .accessibilityIdentifier("export-omissions-details")
                            .popover(isPresented: $showsExportOmissions, arrowEdge: .bottom) {
                                VStack(alignment: .leading, spacing: 8) {
                                    Text("导出未回填详情").font(.headline)
                                    Text(export.lastPathComponent).font(.caption).foregroundStyle(.secondary)
                                    ExportOmissionsView(omissions: reader.lastChapterExportOmissions)
                                }.padding(16).frame(width: 410, height: 285)
                                .background(ReaderPalette.bar).foregroundStyle(ReaderPalette.text)
                            }
                    }
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
            .onChange(of: reader.lastChapterExport) { _, _ in showsExportOmissions = false }
    }
}

import AppKit
import SwiftUI

private typealias ExportState<Value> = SwiftUI.State<Value>

@MainActor struct ReaderExportOptionsView: View {
    private enum Scope: String, CaseIterable {
        case chapter = "整章"
        case selectedPages = "指定页码"
    }

    @ObservedObject var reader: Reader
    @Environment(\.dismiss) private var dismiss
    @StateObject private var session: ExportPreflightSession
    @ExportState private var files: [URL]
    @ExportState private var enhancement: ChapterExport.Enhancement
    @ExportState private var source: ExportSource
    @ExportState private var scope: Scope = .chapter
    @ExportState private var pageInput: String
    @ExportState private var name: String
    @ExportState private var directoryPath: String
    @ExportState private var exporting = false
    @ExportState private var exportError: String?
    @ExportState private var output: URL?
    @ExportState private var omissions: [Int: Int] = [:]

    init(reader: Reader) {
        self.reader = reader
        let options = reader.defaultChapterExportOptions()
        let files = reader.files, enhancement = reader.chapterExportEnhancement
        let currentPage = (reader.selected.flatMap { files.firstIndex(of: $0) } ?? 0) + 1
        _files = ExportState(initialValue: files)
        _enhancement = ExportState(initialValue: enhancement)
        _source = ExportState(initialValue: ExportSource(rawValue: reader.processingMode) ?? .text)
        _pageInput = ExportState(initialValue: String(currentPage))
        _name = ExportState(initialValue: options.folderName)
        _directoryPath = ExportState(initialValue: options.directory.path)
        _session = StateObject(wrappedValue: ExportPreflightSession { number, source in
            try await reader.checkChapterExportPage(files[number - 1], source: source, enhancement: enhancement)
        })
    }

    private var pageSelection: Result<IndexSet, Error> {
        switch scope {
        case .chapter: return .success(IndexSet(integersIn: 1..<(files.count + 1)))
        case .selectedPages: return Result { try ChapterExport.selectedPages(pageInput, pageCount: files.count) }
        }
    }

    private var selection: ExportSelection? {
        (try? pageSelection.get()).map { ExportSelection(source: source, pages: $0) }
    }

    private var folderName: String? { try? ChapterExport.validatedFolderName(name) }

    private var directory: URL? {
        let path = (directoryPath as NSString).expandingTildeInPath
        guard path.hasPrefix("/"), !path.utf8.contains(0) else { return nil }
        return URL(fileURLWithPath: path, isDirectory: true).standardizedFileURL
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let output {
                completed(output)
            } else if exporting {
                Text("正在导出漫画").font(.headline)
                ProgressView(value: Double(reader.queueDone), total: Double(max(reader.queueTotal, 1)))
                Text(reader.message).foregroundStyle(.secondary)
                HStack { Spacer(); Button("取消导出", action: reader.cancel).keyboardShortcut(.cancelAction) }
            } else {
                Text("导出漫画").font(.headline)
                settings
                Divider()
                preflight
                if let exportError {
                    Text(exportError).font(.callout).foregroundStyle(.orange).textSelection(.enabled)
                }
                Divider()
                footer
            }
        }
        .padding(20).frame(width: 610)
        .background(ReaderPalette.bar).foregroundStyle(ReaderPalette.text).tint(ReaderPalette.accent)
        .interactiveDismissDisabled(exporting)
        .onChange(of: selection, initial: true) { _, value in
            exportError = nil
            session.select(value)
        }
        .onChange(of: reader.files) { _, _ in close() }
        .onDisappear { session.stop() }
    }

    private var settings: some View {
        Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 9) {
            GridRow {
                Text("导出页面")
                Picker("导出页面", selection: $scope) {
                    ForEach(Scope.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }.labelsHidden().pickerStyle(.segmented).accessibilityIdentifier("export-page-scope")
            }
            if scope == .selectedPages {
                GridRow {
                    Text("页码")
                    VStack(alignment: .leading, spacing: 3) {
                        TextField("例如 1、3、5–8", text: $pageInput)
                            .textFieldStyle(.roundedBorder).accessibilityIdentifier("export-page-input")
                        if case .failure(let error) = pageSelection {
                            Text(error.localizedDescription).font(.caption).foregroundStyle(.orange)
                        }
                    }
                }
            }
            GridRow {
                Text("本次译文来源")
                VStack(alignment: .leading, spacing: 3) {
                    Picker("本次译文来源", selection: $source) {
                        ForEach(ExportSource.allCases, id: \.self) { Text($0.title).tag($0) }
                    }.labelsHidden().pickerStyle(.segmented).accessibilityIdentifier("export-source")
                    if source.rawValue != reader.processingMode {
                        Text("来源切换仅对本次导出生效。").font(.caption).foregroundStyle(.secondary)
                    }
                }
            }
            GridRow {
                Text("文件夹名称")
                VStack(alignment: .leading, spacing: 3) {
                    TextField("文件夹名称", text: $name).accessibilityIdentifier("export-folder-name")
                    if folderName == nil { Text("请输入有效的文件夹名称。").font(.caption).foregroundStyle(.orange) }
                }
            }
            GridRow {
                Text("保存位置")
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 8) {
                        TextField("保存位置", text: $directoryPath)
                            .help(directoryPath).accessibilityIdentifier("export-directory")
                        Button("选择…", action: chooseDirectory)
                            .buttonStyle(ReaderControlButtonStyle()).accessibilityIdentifier("export-choose-directory")
                    }
                    if directory == nil { Text("请输入完整的文件夹路径。").font(.caption).foregroundStyle(.orange) }
                }
            }
        }.textFieldStyle(.roundedBorder)
    }

    @ViewBuilder private var preflight: some View {
        let report = session.review
        VStack(alignment: .leading, spacing: 7) {
            HStack {
                Text("导出前检查").font(.headline)
                Spacer()
                if session.checking {
                    ProgressView().controlSize(.small)
                    Text("\(report == nil ? "正在检查" : "正在复查") \(session.checked) / \(selection?.pages.count ?? 0)")
                        .font(.caption).foregroundStyle(.secondary).monospacedDigit()
                } else if let selection {
                    Button("重新检查") { session.refresh(selection) }.buttonStyle(.link)
                }
            }
            if let note = session.notice {
                Text(note).foregroundStyle(.orange).accessibilityIdentifier("export-preflight-change")
            }
            if let report, report.selection == selection {
                if !report.missing.isEmpty {
                    Text("第\(ExportReview.pageNumbers(report.missing))页缺少\(source.title)结果。请切换来源或调整页码。")
                        .foregroundStyle(.orange)
                }
                if !report.uncertain.isEmpty {
                    Text("第\(ExportReview.pageNumbers(report.uncertain))页还有未能判定的结果，详见逐页说明。")
                        .foregroundStyle(.orange)
                }
                if !report.unverified.isEmpty {
                    Text("第\(ExportReview.pageNumbers(report.unverified))页检查失败。仍尝试导出时，无法确认这些页是否又有手改或其他变化。")
                        .foregroundStyle(.orange)
                }
                if report.models.count > 1 {
                    Text("所选页使用了 \(report.models.count) 种翻译模型").foregroundStyle(.secondary)
                }
                if report.unknownModels > 0 {
                    Text("有 \(report.unknownModels) 条译文的模型来源未知").foregroundStyle(.secondary)
                }
                if report.details.isEmpty {
                    Text("\(report.pages.count) 页已检查，可使用\(source.title)导出。").foregroundStyle(.secondary)
                } else {
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 8) {
                            ForEach(report.details) { page in
                                ExportReviewRow(page: page, source: source, jump: { jump(to: page.id) })
                                Divider()
                            }
                        }.padding(.trailing, 8)
                    }
                    .frame(height: 186)
                    .id(source).accessibilityIdentifier("export-preflight-pages")
                }
            }
        }.font(.callout).frame(maxWidth: .infinity, alignment: .leading)
    }

    private var footer: some View {
        let report = session.review
        let ready = report != nil && report?.selection == selection && report?.missing.isEmpty == true
        let label = report?.unverified.isEmpty == false || report?.uncertain.isEmpty == false ? "仍尝试导出" :
            (report?.hasSourceWarnings == true ? "按\(source.title)继续导出" : "开始导出")
        return HStack(spacing: 8) {
            if let selection {
                Text("\(selection.pages.count) 页 · PNG").font(.caption).monospacedDigit().foregroundStyle(.secondary)
            }
            Spacer()
            Button("取消", action: close).buttonStyle(ReaderControlButtonStyle()).keyboardShortcut(.cancelAction)
            Button(label, action: confirm).buttonStyle(ReaderPrimaryButtonStyle()).keyboardShortcut(.defaultAction)
                .disabled(!ready || session.checking || folderName == nil || directory == nil || !reader.canEnhanceChapter)
                .accessibilityIdentifier("export-confirm")
        }
    }

    @ViewBuilder private func completed(_ output: URL) -> some View {
        Text("导出完成").font(.headline)
        Text(reader.message)
        if !omissions.isEmpty {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 5) {
                    ForEach(omissions.keys.sorted(), id: \.self) { number in
                        Text("第 \(number) 页 · \(omissions[number] ?? 0) 处未回填，已保留原图")
                    }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }.frame(maxHeight: 190)
        }
        Text(output.path).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
        HStack {
            Button("在访达显示") { NSWorkspace.shared.activateFileViewerSelecting([output]) }
            Spacer()
            Button("完成", action: close).keyboardShortcut(.defaultAction)
        }
    }

    private func jump(to number: Int) {
        guard reader.files == files, files.indices.contains(number - 1) else { return }
        close()
        reader.select(files[number - 1])
    }

    private func close() {
        session.stop()
        dismiss()
    }

    private func chooseDirectory() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.directoryURL = directory ?? reader.defaultChapterExportOptions().directory
        panel.prompt = "选择此位置"
        if panel.runModal() == .OK, let chosen = panel.url { directoryPath = chosen.path }
    }

    private func confirm() {
        guard let selection, reader.canEnhanceChapter, let directory, let folderName else { return }
        exportError = nil
        session.confirm(selection) { report in
            guard report.selection == self.selection, reader.files == files,
                  self.directory == directory, self.folderName == folderName else { return }
            exporting = true
            let options = ChapterExport.Options(pages: selection.pages, folderName: folderName, directory: directory)
            reader.exportChapter(options, reviewed: report, enhancement: enhancement) { outcome in
                exporting = false
                switch outcome {
                case let .completed(directory, counts):
                    omissions = counts
                    output = directory
                case .cancelled:
                    close()
                case .changed(let page):
                    session.refresh(selection, notice: "第 \(page) 页结果已变化，导出已停止。请核对更新后再导出。")
                case .failed(let reason):
                    exportError = reason
                }
            }
        }
    }
}

private struct ExportReviewRow: View {
    let page: ExportReview.Page
    let source: ExportSource
    let jump: () -> Void
    @ExportState private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline) {
                Text("第 \(page.id) 页").fontWeight(.semibold)
                Text(page.labels(source: source).joined(separator: " · ")).foregroundStyle(.secondary)
                Spacer()
                Button("去本页核对", action: jump).buttonStyle(.link)
                    .accessibilityIdentifier("export-jump-\(page.id)")
            }
            switch page.inspection {
            case .failed(let reason):
                Text(reason).textSelection(.enabled).foregroundStyle(.secondary)
            case .checked(let result):
                if result.origin == .derived {
                    Text("视觉辅助沿用普通翻译的识别框和手改。普通机翻不会带入这次导出。")
                }
                if let note = result.note, !note.isEmpty { Text(note).foregroundStyle(.secondary) }
                if !result.analysis.models.isEmpty {
                    Text("模型 · " + result.analysis.models.joined(separator: "、")).font(.caption).textSelection(.enabled)
                }
                if !result.analysis.edits.isEmpty {
                    DisclosureGroup("对照 \(result.analysis.edits.count) 条手改译文", isExpanded: $expanded) {
                        if expanded {
                            LazyVStack(alignment: .leading, spacing: 7) {
                                ForEach(result.analysis.edits.indices, id: \.self) { index in
                                    let edit = result.analysis.edits[index]
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(edit.kind.title).fontWeight(.medium)
                                        Text("原文 · " + edit.source)
                                        Text("\(source.title)（本次）· " + (edit.selected_translation ?? (edit.kind == .different ? "尚无译文" : "无法确定对应译文")))
                                        Text("\(source.other.title) · " + edit.other_translation)
                                    }.textSelection(.enabled)
                                }
                            }.frame(maxWidth: .infinity, alignment: .leading).padding(.top, 3)
                        }
                    }.accessibilityIdentifier("export-edits-\(page.id)")
                }
            }
        }.font(.callout)
    }
}

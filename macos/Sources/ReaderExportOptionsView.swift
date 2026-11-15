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
    @ExportState private var scope: Scope = .chapter
    @ExportState private var pageInput: String
    @ExportState private var name: String
    @ExportState private var directoryPath: String
    private let pageCount: Int

    init(reader: Reader) {
        self.reader = reader
        let options = reader.defaultChapterExportOptions()
        pageCount = reader.files.count
        let currentPage = (reader.selected.flatMap { reader.files.firstIndex(of: $0) } ?? 0) + 1
        _pageInput = ExportState(initialValue: String(currentPage))
        _name = ExportState(initialValue: options.folderName)
        _directoryPath = ExportState(initialValue: options.directory.path)
    }

    private var pageSelection: Result<IndexSet, Error> {
        switch scope {
        case .chapter:
            return .success(IndexSet(integersIn: 1..<(pageCount + 1)))
        case .selectedPages:
            return Result { try ChapterExport.selectedPages(pageInput, pageCount: pageCount) }
        }
    }

    private var folderName: String? { try? ChapterExport.validatedFolderName(name) }

    private var directory: URL? {
        let path = (directoryPath as NSString).expandingTildeInPath
        guard path.hasPrefix("/"), !path.utf8.contains(0) else { return nil }
        return URL(fileURLWithPath: path, isDirectory: true).standardizedFileURL
    }

    var body: some View {
        let selection = pageSelection
        let pages = try? selection.get()
        VStack(alignment: .leading, spacing: 16) {
            Text("导出漫画").font(.headline)
            VStack(alignment: .leading, spacing: 8) {
                Text("导出页面").font(.system(size: 12, weight: .semibold))
                Picker("导出页面", selection: $scope) {
                    ForEach(Scope.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }.labelsHidden().pickerStyle(.segmented).accessibilityIdentifier("export-page-scope")
                if scope == .selectedPages {
                    TextField("页码", text: $pageInput)
                        .textFieldStyle(.roundedBorder).accessibilityIdentifier("export-page-input")
                    Group {
                        if case let .failure(error) = selection {
                            Text(error.localizedDescription)
                        } else {
                            Text("可混合输入单页和范围，如 1、3、5–8。")
                        }
                    }.font(.caption).foregroundStyle(ReaderPalette.secondary)
                }
            }
            VStack(alignment: .leading, spacing: 6) {
                Text("文件夹名称").font(.system(size: 12, weight: .semibold))
                TextField("文件夹名称", text: $name)
                    .textFieldStyle(.roundedBorder).accessibilityIdentifier("export-folder-name")
                if folderName == nil {
                    Text("请输入有效的文件夹名称。").font(.caption).foregroundStyle(ReaderPalette.secondary)
                }
            }
            VStack(alignment: .leading, spacing: 6) {
                Text("保存位置").font(.system(size: 12, weight: .semibold))
                HStack(spacing: 8) {
                    TextField("保存位置", text: $directoryPath)
                        .textFieldStyle(.roundedBorder)
                        .help(directoryPath).accessibilityIdentifier("export-directory")
                    Button("选择…", action: chooseDirectory)
                        .buttonStyle(ReaderControlButtonStyle()).accessibilityIdentifier("export-choose-directory")
                }
                if directory == nil {
                    Text("请输入完整的文件夹路径，或使用「选择…」。")
                        .font(.caption).foregroundStyle(ReaderPalette.secondary)
                }
            }
            Divider().overlay(ReaderPalette.separator)
            HStack(spacing: 8) {
                if let pages {
                    Text("\(pages.count) 页 · PNG").monospacedDigit()
                        .font(.caption).foregroundStyle(ReaderPalette.secondary)
                }
                Spacer()
                Button("取消") { dismiss() }
                    .buttonStyle(ReaderControlButtonStyle()).keyboardShortcut(.cancelAction)
                Button("开始导出", action: startExport)
                    .buttonStyle(ReaderPrimaryButtonStyle()).keyboardShortcut(.defaultAction)
                    .disabled(pages == nil || folderName == nil || directory == nil || !reader.canEnhanceChapter)
                    .accessibilityIdentifier("export-confirm")
            }
        }.padding(20).frame(width: 440)
            .background(ReaderPalette.bar).foregroundStyle(ReaderPalette.text).tint(ReaderPalette.accent)
            .onChange(of: reader.files) { _, _ in dismiss() }
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

    private func startExport() {
        guard let pages = try? pageSelection.get(), !pages.isEmpty,
              let folderName, let directory, reader.canEnhanceChapter else { return }
        reader.exportChapter(ChapterExport.Options(pages: pages, folderName: folderName, directory: directory))
        dismiss()
    }
}

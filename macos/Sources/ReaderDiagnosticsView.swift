import AppKit
import SwiftUI

struct ReaderDiagnosticsView: View {
    @ObservedObject var reader: Reader
    let showDialogue: (String) -> Void

    private var blocks: [TextBlock] { reader.page?.blocks ?? [] }
    private var candidates: [TextBlock] { blocks.filter { !$0.keepsOriginal && !$0.needsOCRReview } }
    private var translated: Int {
        candidates.filter { $0.error.isEmpty && !$0.translation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }.count
    }
    private var problems: [(number: Int, blockID: String, reason: String)] {
        let displayed = Set(reader.readingPresentations.flatMap(\.region.block_ids))
        return blocks.enumerated().compactMap { index, block in
            guard !block.keepsOriginal, let reason = problem(for: block, displayed: displayed) else { return nil }
            return (index + 1, block.id, reason)
        }
    }

    private func problem(for block: TextBlock, displayed: Set<String>) -> String? {
        if block.needsOCRReview { return block.ocr_review?.reason ?? "疑似非文字，已暂停处理" }
        if let issue = block.translationIssue { return issue }
        if let reason = reader.readingPreserved[block.id] { return reason }
        if block.overlay == "skip" || block.text_kind == "noise" { return nil }
        if block.translation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return block.source.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? "日文识别为空" : "缺少中文译文"
        }
        if block.needsReadingOverlay, !displayed.contains(block.id),
           let page = reader.page, let regions = reader.readingRegions {
            return ReadingLayout.failureReason(blockID: block.id, page: page, regions: regions)
        }
        return nil
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("本页诊断").font(.headline)
                    if let selected = reader.selected {
                        Text(selected.lastPathComponent).font(.caption).foregroundStyle(ReaderPalette.secondary)
                    }
                }
                stage("超分", enhancementStatus)
                stage("识别", recognitionStatus)
                stage("翻译", translationStatus)
                stage("回填", layoutStatus)
                let issues = problems
                if !issues.isEmpty {
                    Divider()
                    ForEach(issues, id: \.blockID) { item in
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Button("对白 " + String(format: "%02d", item.number)) { showDialogue(item.blockID) }
                                .buttonStyle(.link).fixedSize()
                                .help("文字框编号 " + item.blockID)
                            Text(item.reason).textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }.font(.caption)
                    }
                }
                if FileManager.default.fileExists(atPath: logURL.path) {
                    Button("在访达显示本机日志") { NSWorkspace.shared.activateFileViewerSelecting([logURL]) }
                        .buttonStyle(.link).font(.caption)
                }
            }.padding(16).frame(maxWidth: .infinity, alignment: .leading)
        }.frame(width: 372).frame(maxHeight: 460)
            .background(ReaderPalette.bar).foregroundStyle(ReaderPalette.text).tint(ReaderPalette.accent)
            .accessibilityIdentifier("page-diagnostics-panel")
    }

    private var logURL: URL { reader.cacheDir.deletingLastPathComponent().appendingPathComponent("native-core.log") }

    private var enhancementStatus: String {
        if reader.preparingImage { return "正在准备增强图像…" }
        let resolution = reader.image?.representations.first.map {
            "\($0.pixelsWide) × \($0.pixelsHigh) px"
        }
        if let warning = reader.enhancementWarning {
            return [resolution, warning].compactMap { $0 }.joined(separator: "\n")
        }
        let engine = reader.srEngine == "metalfx" ? "MetalFX" : "Real-CUGAN"
        let status = reader.displaysEnhancedImage ? engine : "使用原始图像"
        return [status, resolution].compactMap { $0 }.joined(separator: " · ")
    }

    private var recognitionStatus: String {
        if reader.loadingPage { return "正在读取本页结果…" }
        if let error = reader.pageLoadError { return error }
        guard reader.page != nil else {
            return "当前方式下没有本页识别结果。可从工具栏的「…」选择「只识别本页日文」。"
        }
        let review = blocks.filter(\.needsOCRReview).count
        if review > 0 { return "已识别 \(blocks.count - review) 处文字 · \(review) 处疑似非文字，待核对" }
        return blocks.isEmpty ? "识别完成，未找到文字。可用「圈选补识别」在原图上补充对白。" : "已识别 \(blocks.count) 处文字"
    }

    private var translationStatus: String {
        let mode = reader.processingMode == "vision" ? "视觉辅助" : "普通翻译"
        let model = reader.model.isEmpty ? "未选择模型" : reader.model
        let service = "\(mode) · \(reader.connection.name) · \(model)"
        if let selected = reader.selected, let detail = reader.pageProcessingErrors[selected] {
            return service + "\n" + detail
        }
        if reader.engineReady == false, let detail = reader.connectionError {
            return service + "\n" + detail
        }
        let failures = candidates.filter { $0.translationIssue != nil }.count
        let kept = blocks.filter(\.keepsOriginal).count
        let cachedModel = reader.page?.model ?? ""
        let provenance = cachedModel.isEmpty || cachedModel == model ? "" : "\n页面已存的默认模型为 \(cachedModel)"
        return service + "\n已有译文 \(translated) 处 · 失败 \(failures) 处 · 保留原框 \(kept) 处" + provenance
    }

    private var layoutStatus: String {
        if let error = reader.readingLayoutError { return error }
        guard translated > 0 else { return "等待中文译文" }
        guard reader.readingRegions != nil else { return "正在整理中文排版…" }
        let count = Set(reader.readingPresentations.flatMap(\.region.block_ids)).count
        let hint = reader.displayMode == 2 ? "" : "。切换到「中文」查看"
        return "已排版 \(count) 处 · 暂未回填 \(reader.unreadableOverlayCount) 处" + hint
    }

    private func stage(_ label: String, _ detail: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text(label).font(.callout.weight(.semibold)).frame(width: 32, alignment: .leading)
            Text(detail).font(.caption).foregroundStyle(ReaderPalette.secondary).textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

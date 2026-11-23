import Foundation
import Combine

enum ExportSource: String, CaseIterable {
    case text, vision
    var title: String { self == .vision ? "视觉辅助" : "普通翻译" }
    var other: Self { self == .vision ? .text : .vision }
}

struct ExportPageCheck: Decodable, Equatable {
    enum Status: String, Decodable { case ready, missing, uncertain }
    enum Origin: String, Decodable { case existing, seed, legacy, derived, missing }
    struct Analysis: Decodable, Equatable {
        struct Edit: Decodable, Equatable {
            enum Kind: String, Decodable {
                case different, bothEdited = "both_edited", unmatched, needsReview = "needs_review"

                var title: String {
                    switch self {
                    case .different: return "另一来源的手改译文与本次不同"
                    case .bothEdited: return "两边都手改过，译文不同"
                    case .unmatched: return "未找到可靠对应的对白"
                    case .needsReview: return "需人工核对对应关系或回填状态"
                    }
                }
            }
            let kind: Kind
            let source: String
            let other_translation: String
            let selected_translation: String?
        }
        struct Issues: Decodable, Equatable {
            let untranslated, failed, pending, suspect: Int

            var labels: [String] {
                [(untranslated, "条未翻译"), (failed, "条翻译失败"), (pending, "条待核对译文"), (suspect, "处疑似非文字")]
                    .compactMap { $0.0 > 0 ? "\($0.0)\($0.1)" : nil }
            }
        }
        let edits: [Edit]
        let issues: Issues
        let models: [String]
        let unknown_models: Int
    }

    let status: Status
    let origin: Origin
    let other_available: Bool
    let fingerprint: String
    let analysis: Analysis
    let note: String?
}

enum ExportInspection: Equatable {
    case checked(ExportPageCheck)
    case failed(String)

    var check: ExportPageCheck? {
        if case .checked(let result) = self { return result }
        return nil
    }
}

struct ExportSelection: Equatable {
    let source: ExportSource
    let pages: IndexSet
}

struct ExportReview {
    struct Page: Identifiable {
        let id: Int
        let inspection: ExportInspection

        func labels(source: ExportSource) -> [String] {
            guard case .checked(let check) = inspection else { return ["预检失败"] }
            var labels: [String] = []
            switch check.status {
            case .missing:
                labels.append("缺少\(source.title)结果")
                if check.other_available { labels.append("\(source.other.title)有结果") }
            case .uncertain: labels.append("无法预先判定")
            case .ready: break
            }
            if check.origin == .derived { labels.append("沿用普通识别框和手改") }
            let analysis = check.analysis
            if !analysis.edits.isEmpty { labels.append("另一来源有手改 \(analysis.edits.count) 处") }
            labels += analysis.issues.labels
            if analysis.models.count > 1 { labels.append("同页使用了多种模型") }
            if analysis.unknown_models > 0 { labels.append("\(analysis.unknown_models) 条模型未知") }
            return labels
        }
    }

    let selection: ExportSelection
    let pages: [Page]
    let details: [Page]
    let missing: IndexSet
    let uncertain: IndexSet
    let unverified: IndexSet
    let hasSourceWarnings: Bool
    let models: [String]
    let unknownModels: Int
    let fingerprints: [Int: String]

    init(selection: ExportSelection, pages: [Page]) {
        self.selection = selection
        self.pages = pages
        missing = IndexSet(pages.lazy.filter { $0.inspection.check?.status == .missing }.map(\.id))
        uncertain = IndexSet(pages.lazy.filter { $0.inspection.check?.status == .uncertain }.map(\.id))
        unverified = IndexSet(pages.lazy.filter { $0.inspection.check == nil }.map(\.id))
        hasSourceWarnings = pages.contains {
            $0.inspection.check?.origin == .derived || $0.inspection.check?.analysis.edits.isEmpty == false
        }
        let models = Set(pages.flatMap { $0.inspection.check?.analysis.models ?? [] }).sorted()
        self.models = models
        unknownModels = pages.reduce(0) { $0 + ($1.inspection.check?.analysis.unknown_models ?? 0) }
        // Keep source conflicts ahead of per-page model provenance in long chapters.
        func priority(_ page: Page) -> Int {
            guard let check = page.inspection.check, check.status == .ready else { return 0 }
            if !check.analysis.edits.isEmpty || check.origin == .derived { return 0 }
            if !check.analysis.issues.labels.isEmpty || check.note?.isEmpty == false { return 1 }
            if check.analysis.unknown_models > 0 || (models.count > 1 && !check.analysis.models.isEmpty) { return 2 }
            return 3
        }
        details = (0...2).flatMap { rank in pages.filter { priority($0) == rank } }
        fingerprints = Dictionary(uniqueKeysWithValues: pages.compactMap { page -> (Int, String)? in
            guard let check = page.inspection.check else { return nil }
            return (page.id, check.fingerprint)
        })
    }

    static func pageNumbers(_ pages: IndexSet, limit: Int = 8) -> String {
        let ranges = pages.rangeView
        let shown = ranges.prefix(limit).map { range in
            range.count == 1 ? String(range.lowerBound) : "\(range.lowerBound)–\(range.upperBound - 1)"
        }.joined(separator: "、")
        return ranges.count > limit ? shown + " 等" : shown
    }
}

/// A sheet owns one session. Its image list and export enhancement settings are fixed for its lifetime.
@MainActor final class ExportPreflightSession: ObservableObject {
    typealias LoadPage = @MainActor (Int, ExportSource) async throws -> ExportPageCheck

    @Published private(set) var review: ExportReview?
    @Published private(set) var checked = 0
    @Published private(set) var checking = false
    @Published private(set) var notice: String?
    private var task: Task<Void, Never>?
    private var generation = UUID()
    private var cache: [ExportSource: [Int: ExportInspection]] = [:]
    private let load: LoadPage

    init(load: @escaping LoadPage) { self.load = load }

    func stop() {
        generation = UUID()
        task?.cancel()
        task = nil
        checking = false
    }

    func select(_ selection: ExportSelection?) {
        stop()
        review = nil
        notice = nil
        guard let selection, !selection.pages.isEmpty else { return }
        scan(selection, fresh: [], previous: nil)
    }

    func refresh(_ selection: ExportSelection, notice: String? = nil) {
        stop()
        self.notice = notice
        review = nil
        scan(selection, fresh: selection.pages, previous: nil)
    }

    func confirm(_ selection: ExportSelection, proceed: @escaping @MainActor (ExportReview) -> Void) {
        guard !checking, let review, review.selection == selection, review.missing.isEmpty else { return }
        stop()
        notice = nil
        scan(selection, fresh: selection.pages, previous: review, proceed: proceed)
    }

    private func scan(_ selection: ExportSelection, fresh: IndexSet, previous: ExportReview?,
                      proceed: (@MainActor (ExportReview) -> Void)? = nil) {
        let request = generation
        checked = 0
        checking = true
        task = Task {
            var pages: [ExportReview.Page] = []
            pages.reserveCapacity(selection.pages.count)
            var changed = IndexSet()
            var lastProgress = ContinuousClock.now
            for number in selection.pages {
                guard !Task.isCancelled, generation == request else { return }
                let inspection: ExportInspection
                if !fresh.contains(number), let saved = cache[selection.source]?[number] {
                    inspection = saved
                } else {
                    do {
                        let check = try await load(number, selection.source)
                        guard !check.fingerprint.isEmpty else { throw CocoaError(.fileReadCorruptFile) }
                        inspection = .checked(check)
                    } catch { inspection = .failed(error.localizedDescription) }
                    guard !Task.isCancelled, generation == request else { return }
                    cache[selection.source, default: [:]][number] = inspection
                }
                if let previous, previous.pages[pages.count].inspection != inspection { changed.insert(number) }
                pages.append(.init(id: number, inspection: inspection))
                if lastProgress.duration(to: .now) >= .milliseconds(120) {
                    checked = pages.count
                    lastProgress = .now
                    await Task.yield()
                }
            }
            guard !Task.isCancelled, generation == request else { return }
            let latest = ExportReview(selection: selection, pages: pages)
            review = latest
            checked = pages.count
            checking = false
            task = nil
            if !changed.isEmpty {
                notice = "第\(ExportReview.pageNumbers(changed))页的结果或检查状态已变化。请核对更新后再导出。"
            } else if latest.missing.isEmpty {
                proceed?(latest)
            }
        }
    }
}

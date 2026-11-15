import SwiftUI

struct ReaderDraftKey: Hashable {
    let cache: String
    let blockID: String
}

struct ReaderEditDraft: Equatable {
    enum Phase { case editing, saving, saved }
    let originalSource: String
    let originalTranslation: String
    var source: String
    var translation: String
    var phase: Phase = .editing

    init(block: TextBlock) {
        originalSource = block.source; originalTranslation = block.translation
        source = block.source; translation = block.translation
    }

    var changed: Bool { source != originalSource || translation != originalTranslation }
    var sourceChanged: Bool { source != originalSource }
    var savedTranslation: String { sourceChanged ? "" : translation }
}

@MainActor final class ReaderWorkspaceState: ObservableObject {
    @Published var showsInspector = true
    @Published var showsPages = false
    @Published var showsProcessingSettings = false
    @Published var showsExportOptions = false
    @Published private(set) var drafts: [ReaderDraftKey: ReaderEditDraft] = [:]

    func draft(for key: ReaderDraftKey, block: TextBlock) -> ReaderEditDraft {
        drafts[key] ?? ReaderEditDraft(block: block)
    }

    func update(_ key: ReaderDraftKey, block: TextBlock, source: String? = nil, translation: String? = nil) {
        var value = draft(for: key, block: block)
        guard value.phase == .editing else { return }
        if let source { value.source = source }
        if let translation { value.translation = translation }
        if value.changed { drafts[key] = value } else { drafts.removeValue(forKey: key) }
    }

    func beginSave(_ key: ReaderDraftKey) {
        guard var draft = drafts[key], draft.changed else { return }
        draft.phase = .saving; drafts[key] = draft
    }

    func finishSave(_ key: ReaderDraftKey, succeeded: Bool) {
        guard var draft = drafts[key] else { return }
        draft.phase = succeeded ? .saved : .editing
        drafts[key] = draft
    }

    func reconcile(cache: String, page: PageResult) {
        for block in page.blocks {
            let key = ReaderDraftKey(cache: cache, blockID: block.id)
            guard let draft = drafts[key], draft.phase == .saved else { continue }
            if block.source == draft.source && block.translation == draft.savedTranslation {
                drafts.removeValue(forKey: key)
            }
        }
    }
}

enum ReaderCanvasGeometry {
    struct Layout {
        let image: CGSize
        let document: CGSize
        let inset: CGPoint
    }

    static func layout(image: CGSize, viewport: CGSize, zoom: Double) -> Layout {
        let width = max(1, image.width), height = max(1, image.height)
        let fit = min(max(1, viewport.width - 32) / width, max(1, viewport.height - 32) / height)
        let scale = fit * max(0.5, min(3, zoom))
        let fitted = CGSize(width: width * scale, height: height * scale)
        let document = CGSize(width: max(viewport.width, fitted.width + 32),
                              height: max(viewport.height, fitted.height + 32))
        return Layout(image: fitted, document: document,
                      inset: CGPoint(x: (document.width - fitted.width) / 2,
                                     y: (document.height - fitted.height) / 2))
    }
}

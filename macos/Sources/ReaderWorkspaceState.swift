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
    var pageChangedWhileEditing = false

    init(block: TextBlock) {
        originalSource = block.source; originalTranslation = block.translation
        source = block.source; translation = block.translation
    }

    var changed: Bool { source != originalSource || translation != originalTranslation }
    var sourceChanged: Bool { source != originalSource }
    var clearsPreviousTranslation: Bool { sourceChanged && translation == originalTranslation }
    var savedTranslation: String { clearsPreviousTranslation ? "" : translation }

    func refreshed(with block: TextBlock) -> Self {
        guard phase == .editing, block.source != originalSource || block.translation != originalTranslation else { return self }
        var updated = Self(block: block)
        if sourceChanged { updated.source = source }
        if translation != originalTranslation { updated.translation = translation }
        updated.pageChangedWhileEditing = true
        return updated
    }
}

@MainActor final class ReaderWorkspaceState: ObservableObject {
    @Published var showsInspector = true
    @Published var showsPages = false
    @Published var showsProcessingSettings = false
    @Published var showsExportOptions = false
    @Published private(set) var drafts: [ReaderDraftKey: ReaderEditDraft] = [:]

    func draft(for key: ReaderDraftKey, block: TextBlock) -> ReaderEditDraft {
        drafts[key]?.refreshed(with: block) ?? ReaderEditDraft(block: block)
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

    func finishSave(_ key: ReaderDraftKey, succeeded: Bool, visibleCache: String?) {
        guard var draft = drafts[key] else { return }
        if succeeded && visibleCache != key.cache {
            // Returning to this page will load the committed edit or a newer translation.
            drafts.removeValue(forKey: key)
            return
        }
        draft.phase = succeeded ? .saved : .editing
        drafts[key] = draft
    }

    func reconcile(cache: String, page: PageResult) {
        for (key, draft) in drafts where key.cache != cache && draft.phase == .saved {
            drafts.removeValue(forKey: key)
        }
        for block in page.blocks {
            let key = ReaderDraftKey(cache: cache, blockID: block.id)
            guard let draft = drafts[key] else { continue }
            if draft.phase == .editing {
                let updated = draft.refreshed(with: block)
                if updated != draft { drafts[key] = updated.changed ? updated : nil }
            } else if draft.phase == .saved && block.source == draft.source && block.translation == draft.savedTranslation {
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

    static func blockFrame(_ block: TextBlock, page: PageResult, size: CGSize) -> CGRect? {
        guard page.width > 0, page.height > 0, block.box.count == 4,
              block.box.allSatisfy(\.isFinite) else { return nil }
        let scaleX = size.width / Double(page.width), scaleY = size.height / Double(page.height)
        return CGRect(x: block.box[0] * scaleX, y: block.box[1] * scaleY,
                      width: max(12, (block.box[2] - block.box[0]) * scaleX),
                      height: max(12, (block.box[3] - block.box[1]) * scaleY))
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

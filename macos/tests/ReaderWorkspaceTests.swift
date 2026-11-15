import AppKit
import SwiftUI

@main struct ReaderWorkspaceTests {
    @MainActor static func main() {
        let portrait = ReaderCanvasGeometry.layout(image: CGSize(width: 1280, height: 1775),
                                                   viewport: CGSize(width: 900, height: 650), zoom: 1)
        precondition(portrait.image.width <= 868 && portrait.image.height <= 618)
        precondition(abs(portrait.image.width / portrait.image.height - 1280.0 / 1775) < 0.0001)
        precondition(abs(portrait.inset.x * 2 + portrait.image.width - portrait.document.width) < 0.01)
        precondition(abs(portrait.inset.y * 2 + portrait.image.height - portrait.document.height) < 0.01)
        let landscape = ReaderCanvasGeometry.layout(image: CGSize(width: 3000, height: 1200),
                                                    viewport: CGSize(width: 600, height: 650), zoom: 1)
        precondition(landscape.image.width <= 568 && landscape.image.height <= 618)
        let zoomed = ReaderCanvasGeometry.layout(image: CGSize(width: 1280, height: 1775),
                                                 viewport: CGSize(width: 900, height: 650), zoom: 3)
        precondition(abs(zoomed.image.width - portrait.image.width * 3) < 0.01)
        precondition(zoomed.document.width > 900 && zoomed.document.height > 650)
        precondition(zoomed.inset == CGPoint(x: 16, y: 16))
        let empty = ReaderCanvasGeometry.layout(image: .zero, viewport: .zero, zoom: 1)
        precondition(empty.image.width.isFinite && empty.image.height.isFinite)

        let original = TextBlock(id: "1", box: [0, 0, 100, 200], source: "原文", translation: "原译文",
                                 error: "", vertical: true, edited: false)
        let second = TextBlock(id: "2", box: [0, 0, 100, 200], source: "別の原文", translation: "另一条",
                               error: "", vertical: true, edited: false)
        let workspace = ReaderWorkspaceState()
        let text = ReaderDraftKey(cache: "/text/page-a.json", blockID: "1")
        let textSecond = ReaderDraftKey(cache: "/text/page-a.json", blockID: "2")
        let otherPage = ReaderDraftKey(cache: "/text/page-b.json", blockID: "1")
        let vision = ReaderDraftKey(cache: "/vision/page-a.json", blockID: "1")
        let enhanced = ReaderDraftKey(cache: "/enhanced/page-a.json", blockID: "1")
        workspace.update(text, block: original, translation: "未保存的修正")
        precondition(workspace.draft(for: text, block: original).translation == "未保存的修正")
        precondition(workspace.draft(for: textSecond, block: second).translation == "另一条")
        for key in [otherPage, vision, enhanced] {
            precondition(!workspace.draft(for: key, block: original).changed)
        }
        workspace.beginSave(text)
        workspace.update(text, block: original, translation: "不应覆盖正在保存的内容")
        precondition(workspace.draft(for: text, block: original).translation == "未保存的修正")
        workspace.finishSave(text, succeeded: false)
        precondition(workspace.draft(for: text, block: original).phase == .editing)
        precondition(workspace.draft(for: text, block: original).translation == "未保存的修正")
        workspace.beginSave(text)
        workspace.finishSave(text, succeeded: true)
        workspace.reconcile(cache: text.cache, page: page([original]))
        precondition(workspace.drafts[text] != nil, "An older reload must not discard the saved draft before it arrives")
        var updated = original; updated.translation = "未保存的修正"
        workspace.reconcile(cache: text.cache, page: page([updated]))
        precondition(workspace.drafts[text] == nil)

        workspace.update(text, block: updated, source: "修正した原文")
        workspace.beginSave(text); workspace.finishSave(text, succeeded: true)
        var sourceSaved = updated; sourceSaved.source = "修正した原文"; sourceSaved.translation = ""
        workspace.reconcile(cache: text.cache, page: page([sourceSaved]))
        precondition(workspace.drafts[text] == nil, "A successful source edit clears the obsolete translation")
        workspace.update(text, block: sourceSaved, translation: "临时草稿")
        workspace.update(text, block: sourceSaved, translation: "")
        precondition(workspace.drafts[text] == nil, "Undoing an edit returns to the clean state")
        print("Workspace checks passed: whole-page fit, centering, zoom, draft isolation, failed save recovery, stale reload, and source-edit semantics")
    }

    private static func page(_ blocks: [TextBlock]) -> PageResult {
        PageResult(version: 1, width: 1280, height: 1775, blocks: blocks, model: "test", ocr: "test")
    }
}

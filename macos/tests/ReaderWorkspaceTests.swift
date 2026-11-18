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
        workspace.finishSave(text, succeeded: false, visibleCache: otherPage.cache)
        precondition(workspace.draft(for: text, block: original).phase == .editing)
        precondition(workspace.draft(for: text, block: original).translation == "未保存的修正")
        workspace.beginSave(text)
        workspace.finishSave(text, succeeded: true, visibleCache: text.cache)
        workspace.reconcile(cache: text.cache, page: page([original]))
        precondition(workspace.drafts[text] != nil, "An older reload must not discard the saved draft before it arrives")
        var updated = original; updated.translation = "未保存的修正"
        workspace.reconcile(cache: text.cache, page: page([updated]))
        precondition(workspace.drafts[text] == nil)

        workspace.update(text, block: updated, source: "修正した原文")
        workspace.beginSave(text); workspace.finishSave(text, succeeded: true, visibleCache: text.cache)
        var sourceSaved = updated; sourceSaved.source = "修正した原文"; sourceSaved.translation = ""
        workspace.reconcile(cache: text.cache, page: page([sourceSaved]))
        precondition(workspace.drafts[text] == nil, "A successful source edit clears the obsolete translation")
        workspace.update(text, block: updated, source: sourceSaved.source)
        workspace.beginSave(text)
        workspace.finishSave(text, succeeded: true, visibleCache: otherPage.cache)
        var translatedOffscreen = sourceSaved
        translatedOffscreen.translation = "页面再次打开之前模型已生成的新译文"
        workspace.reconcile(cache: text.cache, page: page([translatedOffscreen]))
        let reopened = workspace.draft(for: text, block: translatedOffscreen)
        precondition(reopened.phase == .editing && reopened.translation == translatedOffscreen.translation,
                     "A confirmed offscreen edit must not lock a newer translation on return")
        workspace.update(text, block: updated, source: sourceSaved.source)
        workspace.beginSave(text)
        workspace.finishSave(text, succeeded: true, visibleCache: text.cache)
        workspace.reconcile(cache: otherPage.cache, page: page([second]))
        precondition(workspace.drafts[text] == nil, "Navigating away retires an acknowledged draft awaiting reload")
        workspace.update(textSecond, block: second, source: "新しい原文", translation: "核对后的新译文")
        precondition(workspace.draft(for: textSecond, block: second).savedTranslation == "核对后的新译文",
                     "Editing both languages must keep the newly typed translation")
        workspace.update(text, block: sourceSaved, translation: "临时草稿")
        workspace.update(text, block: sourceSaved, translation: "")
        precondition(workspace.drafts[text] == nil, "Undoing an edit returns to the clean state")
        workspace.update(text, block: original, translation: "正在核对的译文")
        var corrected = original
        corrected.source = "訂正された原文"
        workspace.reconcile(cache: text.cache, page: page([corrected]))
        let refreshed = workspace.draft(for: text, block: corrected)
        precondition(refreshed.source == corrected.source && !refreshed.sourceChanged)
        precondition(refreshed.savedTranslation == "正在核对的译文" && refreshed.pageChangedWhileEditing)
        corrected.translation = refreshed.savedTranslation
        workspace.reconcile(cache: text.cache, page: page([corrected]))
        precondition(workspace.drafts[text] == nil, "A reload that already includes the draft must clear it")
        print("Workspace checks passed: whole-page fit, centering, zoom, draft isolation, failed save recovery, stale reload, offscreen save, and source-edit semantics")
    }

    private static func page(_ blocks: [TextBlock]) -> PageResult {
        PageResult(version: 1, width: 1280, height: 1775, blocks: blocks, model: "test", ocr: "test")
    }
}

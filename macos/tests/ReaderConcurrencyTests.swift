import AppKit
import Foundation

/// A local JSONL peer supplies controlled completion order without a model or a network listener.
@main struct ReaderConcurrencyTests {
    @MainActor static func main() async throws {
        let fm = FileManager.default
        let root = fm.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let peer = root.appendingPathComponent("core/target/release/koma-core")
        try fm.createDirectory(at: peer.deletingLastPathComponent(), withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: root) }
        try #"""
#!/usr/bin/env python3
import json, sys, time
from pathlib import Path
root = Path(__file__).resolve().parents[3]
pages = {}
pending_edit = pending_job = None

def emit(request, event='result', **fields):
    print(json.dumps(dict(id=request['id'], event=event, **fields)), flush=True)

def page_reply(request):
    image = request['image']
    block = pages.setdefault(image, dict(id='1', box=[0, 0, 1, 1], source='原文',
                                         translation='旧译文', error='', vertical=False, edited=False))
    if request['op'] == 'edit':
        block.update(request['values'])
    return dict(cache=image + '.json', revision=json.dumps(block, sort_keys=True),
                page=dict(version=1, width=1, height=1, blocks=[block], model='', ocr=image))

for line in sys.stdin:
    request = json.loads(line)
    op = request['op']
    if op == 'edit' and (root / 'delay-edit').exists():
        pending_edit = request
        (root / 'edit-started').touch()
        continue
    if op == 'process' and (root / 'delay-process').exists():
        pending_job = request
        emit(request, 'page_started', image=request['images'][0])
        continue
    if op == 'cancel' and pending_job and request['target'] == pending_job['id']:
        image = pending_job['images'][0]
        pages[image]['translation'] = '停止前已保存'
        emit(pending_job, 'checkpoint', image=image)
        emit(pending_job, 'cancelled')
        pending_job = None
    if op in ('load_page', 'edit'):
        if op == 'load_page':
            (root / 'last-page-engine').write_text(request['super_resolution']['engine'])
            if (root / 'fail-read').exists():
                emit(request, 'error', error='fixture read failure')
                continue
        emit(request, result=page_reply(request))
        if op == 'load_page' and pending_edit and request['image'] == pending_edit['image']:
            emit(pending_edit, result=page_reply(pending_edit))
            pending_edit = None
        continue
    elif op == 'display':
        result = dict(reply=dict(version=3, regions=[]))
    elif op == 'models':
        with (root / 'model-requests').open('a') as counter:
            counter.write('1\n')
        deadline = time.monotonic() + 3
        while not (root / 'finish-probe').exists() and time.monotonic() < deadline:
            time.sleep(.01)
        result = dict(models=['fixture-model'])
    else:
        result = {}
    emit(request, result=result)
"""#.write(to: peer, atomically: true, encoding: .utf8)
        try fm.setAttributes([.posixPermissions: 0o700], ofItemAtPath: peer.path)
        let suite = UUID().uuidString
        let defaults = UserDefaults(suiteName: suite)!
        let reader = Reader(root: root, cacheDir: root.appendingPathComponent("pages"), defaults: defaults,
                            configurationStore: TranslationLocalConfigStore(url: root.appendingPathComponent("config.json")))
        defer { reader.shutdown(); defaults.removePersistentDomain(forName: suite) }
        func wait(_ condition: () -> Bool) async throws {
            for _ in 0..<500 {
                if condition() { return }
                try await Task.sleep(for: .milliseconds(20))
            }
            throw NativeCoreService.Failure(message: "Timed out waiting for reader: " + (reader.error ?? reader.message))
        }
        let first = root.appendingPathComponent("first.png"), second = root.appendingPathComponent("second.png")
        let pixel = Data(base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4//8/AAX+Av4N70a4AAAAAElFTkSuQmCC")!
        for file in [first, second] { try pixel.write(to: file) }
        reader.files = [first, second]
        reader.select(first)
        try await wait { reader.readingRegions != nil }
        reader.saveEdit(id: "1", source: "日本語修正", translation: "同步修订的中文")
        try await wait { !reader.busy }
        precondition(reader.page?.blocks.first?.translation == "同步修订的中文" && reader.page?.blocks.first?.edited == true)
        reader.saveEdit(id: "1", source: "再修正", translation: "同步修订的中文")
        try await wait { !reader.busy }
        precondition(reader.page?.blocks.first?.translation == "" && reader.page?.blocks.first?.edited == false)
        reader.run(operation: "ocr")
        reader.select(second)
        reader.cancel()
        try await wait { !reader.busy && reader.page?.ocr == second.path && reader.readingRegions != nil }

        var regressions: [String] = []
        try Data().write(to: root.appendingPathComponent("delay-edit"))
        reader.saveEdit(id: "1", source: "原文", translation: "往返翻页后的修订")
        try await wait { fm.fileExists(atPath: root.appendingPathComponent("edit-started").path) }
        reader.move(-1)
        try await wait { reader.page?.ocr == first.path }
        reader.move(1)
        try await wait { !reader.busy && !reader.loadingPage }
        if reader.page?.blocks.first?.translation != "往返翻页后的修订" {
            regressions.append("Returning to a page during a save left the editor on the old revision")
        }

        try Data().write(to: root.appendingPathComponent("delay-process"))
        reader.run(operation: "ocr")
        try await wait { reader.message.contains("正在处理") }
        reader.cancel()
        precondition(reader.busy && reader.message.contains("正在停止"))
        try await wait { !reader.busy && !reader.loadingPage }
        precondition(reader.message.contains("已停止"))
        if reader.page?.blocks.first?.translation != "停止前已保存" {
            regressions.append("A checkpoint delivered after pressing Stop did not reach the reader")
        }

        reader.enhanceChapter()
        reader.cancel()
        try await wait { !reader.busy && !reader.loadingPage }
        if reader.page?.ocr != second.path {
            regressions.append("Stopping chapter enhancement cleared the displayed translation")
        }
        reader.select(second)
        reader.cancel()
        try await wait { !reader.loadingPage }
        let pageEngine = try String(contentsOf: root.appendingPathComponent("last-page-engine"), encoding: .utf8)
        if reader.page?.ocr != second.path || reader.displaysEnhancedImage || reader.preparingImage || reader.srEngine != "metalfx" || pageEngine != "off" {
            regressions.append("Stopping page enhancement failed to restore the cached translation on the original image")
        }

        let empty = root.appendingPathComponent("empty")
        try fm.createDirectory(at: empty, withIntermediateDirectories: false)
        reader.loadFolder(empty)
        reader.run(operation: "ocr", batch: true)
        precondition(!reader.busy, "A folder change must prevent a queue starting on the old chapter")
        try await wait { !reader.openingFolder }
        precondition(reader.selected == second && reader.error != nil && !reader.message.contains("正在读取"))

        let shortTIFF = empty.appendingPathComponent("01.TIF")
        try NSBitmapImageRep(data: pixel)!.tiffRepresentation!.write(to: shortTIFF)
        reader.loadFolder(empty)
        try await wait { !reader.openingFolder && !reader.loadingPage }
        precondition(reader.files.map(\.lastPathComponent) == ["01.TIF"] && reader.selected == reader.files.first && reader.image != nil,
                     "A chapter of .tif pages should open like .tiff pages: \(reader.files), \(reader.error ?? reader.message)")

        try Data().write(to: root.appendingPathComponent("fail-read"))
        reader.select(first)
        try await wait { !reader.loadingPage }
        if reader.page != nil || reader.pageLoadError == nil || reader.message.contains("正在读取") {
            regressions.append("A failed page load left a reading-in-progress status")
        }
        let firstProbe = Task { await reader.refreshModels() }
        let counter = root.appendingPathComponent("model-requests")
        try await wait { fm.fileExists(atPath: counter.path) }
        _ = await reader.refreshModels()
        try Data().write(to: root.appendingPathComponent("finish-probe"))
        let ready = await firstProbe.value
        let requests = try String(contentsOf: counter, encoding: .utf8)
        precondition(ready && reader.engineReady && requests == "1\n", "Overlapping ticks must preserve the active probe")
        guard regressions.isEmpty else {
            throw NativeCoreService.Failure(message: regressions.joined(separator: "\n"))
        }
        print("Passed: bilingual edits, navigation during save, cancellation checkpoints, chapter and page enhancement cancellation, pending folder change, overlapping model discovery")
    }
}

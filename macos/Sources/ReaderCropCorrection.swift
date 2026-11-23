import AppKit
import SwiftUI

private typealias CropState<Value> = SwiftUI.State<Value>

struct ReaderCropSession: Identifiable {
    enum Phase { case selecting, recognizing, reviewing, applying }
    enum Placement: String { case prepend, append, new }

    let id = UUID()
    let image: URL
    let cache: URL
    let revision: String?
    let original: NSImage
    let canvas: CGSize
    var phase = Phase.selecting
    var box: [Int] = []
    var source = ""
    var issue: String?
    var outdated = false
    var placement: Placement
    var target: String?

    static func selection(from start: CGPoint, to end: CGPoint, displayed: CGSize, canvas: CGSize) -> [Int]? {
        guard displayed.width > 0, displayed.height > 0, canvas.width > 0, canvas.height > 0 else { return nil }
        let a = min(max(0, min(start.x, end.x)), displayed.width)
        let b = min(max(0, min(start.y, end.y)), displayed.height)
        let c = min(max(0, max(start.x, end.x)), displayed.width)
        let d = min(max(0, max(start.y, end.y)), displayed.height)
        let box = [Int(floor(a * canvas.width / displayed.width)), Int(floor(b * canvas.height / displayed.height)),
                   Int(ceil(c * canvas.width / displayed.width)), Int(ceil(d * canvas.height / displayed.height))]
        return box[2] - box[0] >= 4 && box[3] - box[1] >= 4 ? box : nil
    }
}

/// The native scroll view hosts this surface over the original image. Tracking
/// mouse-down through mouse-up keeps the selection independent of zoom and pan.
struct ReaderCropSelectionView: NSViewRepresentable {
    let session: ReaderCropSession
    let select: ([Int]) -> Void

    func makeNSView(context: Context) -> Surface {
        let view = Surface()
        view.setAccessibilityElement(true)
        view.setAccessibilityRole(.group)
        view.setAccessibilityLabel("在原图上拖动，框住一列或一行漏掉的日文")
        view.setAccessibilityIdentifier("crop-selection-surface")
        return view
    }

    func updateNSView(_ view: Surface, context: Context) {
        view.update(id: session.id, canvas: session.canvas, select: select)
    }

    final class Surface: NSView {
        override var isFlipped: Bool { true }
        private var sessionID: UUID?
        private var canvas = CGSize.zero
        private var start: CGPoint?
        private var end: CGPoint?
        private var select: (([Int]) -> Void)?

        func update(id: UUID, canvas: CGSize, select: @escaping ([Int]) -> Void) {
            if sessionID != id { start = nil; end = nil; needsDisplay = true }
            sessionID = id
            self.canvas = canvas
            self.select = select
        }

        override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

        override func resetCursorRects() {
            super.resetCursorRects()
            addCursorRect(visibleRect, cursor: .crosshair)
        }

        override func mouseDown(with event: NSEvent) {
            start = convert(event.locationInWindow, from: nil)
            end = start
            needsDisplay = true
        }

        override func mouseDragged(with event: NSEvent) {
            end = convert(event.locationInWindow, from: nil)
            needsDisplay = true
        }

        override func mouseUp(with event: NSEvent) {
            let first = start
            start = nil
            end = nil
            needsDisplay = true
            guard let first, let box = ReaderCropSession.selection(from: first, to: convert(event.locationInWindow, from: nil),
                                                                   displayed: bounds.size, canvas: canvas) else { return }
            select?(box)
        }

        override func draw(_ dirtyRect: NSRect) {
            guard let start, let end else { return }
            let rect = CGRect(x: start.x, y: start.y, width: end.x - start.x, height: end.y - start.y).standardized
            let path = NSBezierPath(rect: rect)
            NSColor.controlAccentColor.withAlphaComponent(0.14).setFill()
            path.fill()
            NSColor.controlAccentColor.setStroke()
            path.lineWidth = 2
            path.stroke()
        }
    }
}

struct ReaderCropReviewView: View {
    @ObservedObject var reader: Reader
    @ObservedObject var workspace: ReaderWorkspaceState
    @CropState private var translation = ""

    var body: some View {
        if let session = reader.cropSession {
            let waiting = session.phase != .reviewing
            let blocks = (reader.page?.blocks ?? []).filter { !$0.keepsOriginal }
            let target = blocks.first { $0.id == session.target }
            let canMerge = target.map { block in
                !workspace.draft(for: ReaderDraftKey(cache: session.cache.path, blockID: block.id), block: block).changed
            } ?? false
            let eligible = !waiting && !session.outdated && !session.source.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                && (session.placement == .new || canMerge)
            VStack(alignment: .leading, spacing: 12) {
                Text("圈选补识别").font(.headline)
                cropPreview(session).frame(height: 96)
                if session.phase == .recognizing {
                    HStack { ProgressView().controlSize(.small); Text("正在识别选中的日文…") }
                }
                if let issue = session.issue { Text(issue).font(.caption).foregroundStyle(.orange).textSelection(.enabled) }
                if session.phase != .recognizing {
                    TextField("选区日文", text: Binding(get: { reader.cropSession?.source ?? "" },
                                                     set: { reader.cropSession?.source = $0 }), axis: .vertical)
                        .lineLimit(1...4).textFieldStyle(.roundedBorder).accessibilityIdentifier("crop-source")
                    if !blocks.isEmpty {
                        Picker("保存到", selection: Binding(get: { reader.cropSession?.target ?? "" }, set: { id in
                            reader.cropSession?.target = id.isEmpty ? nil : id
                            reader.cropSession?.placement = id.isEmpty ? .new : .prepend
                            translation = ""
                        })) {
                            Text("新增对白").tag("")
                            ForEach(blocks) { block in Text("对白 " + block.id + "　" + String(block.source.prefix(18))).tag(block.id) }
                        }.accessibilityIdentifier("crop-target")
                    }
                    if let target, session.placement != .new {
                        Picker("拼接顺序", selection: Binding(get: { reader.cropSession?.placement ?? .prepend },
                                                         set: { reader.cropSession?.placement = $0 })) {
                            Text("加在原文前").tag(ReaderCropSession.Placement.prepend)
                            Text("加在原文后").tag(ReaderCropSession.Placement.append)
                        }.pickerStyle(.segmented)
                        Text(session.placement == .prepend ? session.source + target.source : target.source + session.source)
                            .font(.callout).textSelection(.enabled)
                        if !target.translation.isEmpty {
                            Text("已有译文　" + target.translation).font(.caption).textSelection(.enabled)
                        }
                        if !canMerge { Text("这条对白有未保存的修改，请先保存草稿或选其他对白。").font(.caption) }
                    }
                    TextField("完整中文译文（可选）", text: $translation, axis: .vertical)
                        .textFieldStyle(.roundedBorder).lineLimit(1...4).accessibilityIdentifier("crop-translation")
                    if translation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        Text("仅保存日文后，核对或重新翻译这条对白即可回填。")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
                HStack {
                    Button(session.outdated ? "关闭并重新载入" : "取消", action: reader.dismissCropReview).keyboardShortcut(.cancelAction)
                        .disabled(session.phase == .applying)
                    if !waiting && !session.outdated { Button("重选范围") { translation = ""; reader.reselectCrop() } }
                    Spacer()
                    if session.phase != .recognizing {
                        Button("保存") { reader.applyCrop(translation: translation) }
                            .disabled(!eligible).keyboardShortcut(.defaultAction).accessibilityIdentifier("crop-apply")
                    }
                }
            }.padding(20).frame(width: 440)
                .interactiveDismissDisabled(true)
        }
    }

    @ViewBuilder private func cropPreview(_ session: ReaderCropSession) -> some View {
        if session.box.count == 4 {
            GeometryReader { proxy in
                let b = session.box
                let w = CGFloat(b[2] - b[0]), h = CGFloat(b[3] - b[1])
                let scale = min(proxy.size.width / w, proxy.size.height / h)
                Image(nsImage: session.original).resizable()
                    .frame(width: session.canvas.width * scale, height: session.canvas.height * scale)
                    .offset(x: -CGFloat(b[0]) * scale, y: -CGFloat(b[1]) * scale)
                    .frame(width: w * scale, height: h * scale, alignment: .topLeading).clipped()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }
}

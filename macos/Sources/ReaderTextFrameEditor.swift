import AppKit
import SwiftUI

/// The native scroll view and this surface use the same initial mouse hit. Mouse
/// drags starting on editable text stay with the frame even after crossing it.
struct ReaderTextFrameEditor: NSViewRepresentable {
    struct Target {
        let id: String
        let frame: CGRect
    }
    enum Edge { case move, left, right }
    let targets: [Target]
    let pageSize: CGSize
    let identity: String
    let selectedID: String?
    let editable: Bool
    let invalidPreview: Bool
    let select: (String) -> Void
    let update: (String, CGRect, Bool) -> Void
    let cancel: () -> Void

    func makeNSView(context: Context) -> Surface { Surface() }
    func updateNSView(_ view: Surface, context: Context) {
        view.configure(self)
    }

    final class Surface: NSView {
        override var isFlipped: Bool { true }
        private var editor: ReaderTextFrameEditor?
        private struct Drag {
            let id: String
            let original: CGRect
            let edge: Edge
            let start: CGPoint
            let scale: CGSize
            let canEdit: Bool
            var latest: CGRect?
        }
        private var drag: Drag?
        var reservesMouseDrag: Bool { editor?.editable == true }
        private var scale: CGSize {
            guard let editor else { return .zero }
            return CGSize(width: bounds.width / editor.pageSize.width, height: bounds.height / editor.pageSize.height)
        }

        func configure(_ editor: ReaderTextFrameEditor) {
            if self.editor?.identity != editor.identity || self.editor?.pageSize != editor.pageSize {
                drag = nil
            }
            self.editor = editor
            needsDisplay = true
            window?.invalidateCursorRects(for: self)
            toolTip = editor.selectedID == nil ? "单击译文定位对白" : "拖动排字框调整位置，拖动左右手柄调整宽度"
        }

        private func displayRect(_ target: Target) -> CGRect {
            let rect = drag?.id == target.id ? drag?.latest ?? target.frame : target.frame
            return rect.applying(CGAffineTransform(scaleX: scale.width, y: scale.height))
        }
        private func handle(_ edge: Edge, in rect: CGRect) -> CGRect {
            // Put the controls just outside a narrow frame so its center remains draggable.
            let outset = max(0, (22 - rect.width) / 2)
            let x = edge == .left ? rect.minX - outset : rect.maxX + outset
            return CGRect(x: x - 5, y: rect.midY - 9, width: 10, height: 18)
        }
        private func hit(_ point: CGPoint) -> (Target, Edge)? {
            guard let editor else { return nil }
            if editor.editable, let selected = editor.targets.first(where: { $0.id == editor.selectedID }) {
                let rect = displayRect(selected)
                for edge in [Edge.left, .right] where handle(edge, in: rect).contains(point) { return (selected, edge) }
                if rect.contains(point) { return (selected, .move) }
            }
            return editor.targets.reversed().first { displayRect($0).contains(point) }.map { ($0, .move) }
        }

        override func hitTest(_ point: NSPoint) -> NSView? {
            guard !isHidden, let parent = superview, hit(convert(point, from: parent)) != nil else { return nil }
            return self
        }
        override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

        override func draw(_ dirtyRect: NSRect) {
            guard let editor, editor.editable, let target = editor.targets.first(where: { $0.id == editor.selectedID }) else { return }
            let rect = displayRect(target)
            let color = editor.invalidPreview ? NSColor.systemOrange : NSColor.controlAccentColor
            color.setStroke()
            let path = NSBezierPath(rect: rect)
            path.lineWidth = 1.5
            path.stroke()
            for edge in [Edge.left, .right] {
                let grip = handle(edge, in: rect).insetBy(dx: 2, dy: 3)
                let shape = NSBezierPath(roundedRect: grip, xRadius: 2.5, yRadius: 2.5)
                color.setFill()
                shape.fill()
                NSColor.white.setStroke()
                shape.lineWidth = 1
                shape.stroke()
            }
        }

        override func resetCursorRects() {
            super.resetCursorRects()
            guard let editor else { return }
            for target in editor.targets { addCursorRect(displayRect(target), cursor: editor.editable ? .openHand : .pointingHand) }
            if editor.editable, let selected = editor.targets.first(where: { $0.id == editor.selectedID }) {
                let rect = displayRect(selected)
                for edge in [Edge.left, .right] { addCursorRect(handle(edge, in: rect), cursor: .resizeLeftRight) }
            }
        }

        override func mouseDown(with event: NSEvent) {
            guard let editor, let (target, edge) = hit(convert(event.locationInWindow, from: nil)) else { return }
            drag = Drag(id: target.id, original: target.frame, edge: edge, start: event.locationInWindow,
                        scale: scale, canEdit: editor.editable)
            // When opening the inspector the canvas gets narrower. Open it on mouse-up in that case.
            if editor.editable { editor.select(target.id) }
        }

        private func proposedFrame(event: NSEvent, drag: Drag, page: CGSize) -> CGRect? {
            let dx = event.locationInWindow.x - drag.start.x
            let dy = drag.start.y - event.locationInWindow.y
            guard drag.canEdit, drag.scale.width > 0, drag.scale.height > 0,
                  drag.latest != nil || hypot(dx, dy) >= 2 else { return nil }
            return ReaderTextFrameEditor.adjusted(drag.original, edge: drag.edge, dx: dx / drag.scale.width,
                                                  dy: dy / drag.scale.height, page: page)
        }
        override func mouseDragged(with event: NSEvent) {
            guard let editor, let drag, let frame = proposedFrame(event: event, drag: drag, page: editor.pageSize),
                  frame != drag.latest else { return }
            self.drag?.latest = frame
            needsDisplay = true
            (drag.edge == .move ? NSCursor.closedHand : .resizeLeftRight).set()
            editor.update(drag.id, frame, false)
        }
        override func mouseUp(with event: NSEvent) {
            guard let editor, let drag else { return }
            let frame = proposedFrame(event: event, drag: drag, page: editor.pageSize)
            self.drag = nil
            if let frame, frame != drag.original { editor.update(drag.id, frame, true) }
            else {
                editor.cancel()
                let distance = hypot(event.locationInWindow.x - drag.start.x, event.locationInWindow.y - drag.start.y)
                if distance < 3 { editor.select(drag.id) }
            }
            window?.invalidateCursorRects(for: self)
            needsDisplay = true
        }
        override func cancelOperation(_ sender: Any?) {
            drag = nil
            editor?.cancel()
            needsDisplay = true
        }
    }

    static func adjusted(_ original: CGRect, edge: Edge, dx: CGFloat, dy: CGFloat, page: CGSize) -> CGRect {
        let rect = original.intersection(CGRect(origin: .zero, size: page))
        guard !rect.isNull, !rect.isEmpty else { return original }
        let minimum = min(12, rect.width)
        switch edge {
        case .move:
            return CGRect(x: min(max(0, rect.minX + dx), page.width - rect.width),
                          y: min(max(0, rect.minY + dy), page.height - rect.height), width: rect.width, height: rect.height)
        case .left:
            let left = max(0, min(rect.maxX - minimum, rect.minX + dx))
            return CGRect(x: left, y: rect.minY, width: rect.maxX - left, height: rect.height)
        case .right:
            let right = min(page.width, max(rect.minX + minimum, rect.maxX + dx))
            return CGRect(x: rect.minX, y: rect.minY, width: right - rect.minX, height: rect.height)
        }
    }
}

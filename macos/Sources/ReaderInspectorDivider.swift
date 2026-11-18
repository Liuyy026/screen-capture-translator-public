import AppKit
import SwiftUI

struct ReaderInspectorDivider: NSViewRepresentable {
    enum Drag {
        case began
        case changed(CGFloat)
        case ended(CGFloat)
    }

    let onDrag: (Drag) -> Void

    func makeNSView(context: Context) -> Handle {
        let view = Handle()
        view.setAccessibilityElement(true)
        view.setAccessibilityRole(.splitter)
        view.setAccessibilityOrientation(.vertical)
        view.setAccessibilityLabel("调整对白栏宽度")
        view.setAccessibilityIdentifier("inspector-divider")
        return view
    }

    func updateNSView(_ nsView: Handle, context: Context) {
        nsView.onDrag = onDrag
    }

    final class Handle: NSView {
        var onDrag: ((Drag) -> Void)?
        private var startX: CGFloat?

        private var resizeCursor: NSCursor {
            if #available(macOS 15.0, *) { return .columnResize }
            return .resizeLeftRight
        }

        override func resetCursorRects() {
            addCursorRect(bounds, cursor: resizeCursor)
        }

        override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

        override func mouseDown(with event: NSEvent) {
            startX = event.locationInWindow.x
            resizeCursor.set()
            onDrag?(.began)
        }

        override func mouseDragged(with event: NSEvent) {
            guard let startX else { return }
            resizeCursor.set()
            onDrag?(.changed(event.locationInWindow.x - startX))
        }

        override func mouseUp(with event: NSEvent) {
            guard let startX else { return }
            self.startX = nil
            onDrag?(.ended(event.locationInWindow.x - startX))
            if !bounds.contains(convert(event.locationInWindow, from: nil)) { NSCursor.arrow.set() }
        }
    }
}

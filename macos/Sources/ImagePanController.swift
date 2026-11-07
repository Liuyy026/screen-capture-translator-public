import AppKit
import SwiftUI

/// NSScrollView owns the position; the controller only stores an active drag's
/// starting offset, so scrolling and mouse panning always share one position.
@MainActor final class ImagePanController: ObservableObject {
    private weak var scrollView: NSScrollView?
    private var dragOrigin: NSPoint?

    func attach(to scrollView: NSScrollView?) {
        guard self.scrollView !== scrollView else { return }
        endDrag()
        self.scrollView = scrollView
    }

    func detach(from scrollView: NSScrollView?) {
        if self.scrollView === scrollView { attach(to: nil) }
    }

    func drag(translation: CGSize) {
        guard let scrollView else { return }
        let clip = scrollView.contentView
        let start = dragOrigin ?? clip.bounds.origin
        dragOrigin = start
        let yDirection: CGFloat = clip.isFlipped ? 1 : -1
        var target = clip.bounds
        target.origin = NSPoint(x: start.x - translation.width,
                                y: start.y - translation.height * yDirection)
        clip.scroll(to: clip.constrainBoundsRect(target).origin)
        scrollView.reflectScrolledClipView(clip)
        NSCursor.closedHand.set()
    }

    func endDrag() {
        if dragOrigin != nil { NSCursor.openHand.set() }
        dragOrigin = nil
    }
}

/// Own the image's native scroll container rather than probing SwiftUI's
/// private scroll-view hierarchy, which can change during zoom and page updates.
struct PannableImageScrollView<Content: View>: NSViewRepresentable {
    let controller: ImagePanController
    let documentSize: CGSize
    let pageID: URL?
    @ViewBuilder var content: () -> Content

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = ImageScrollView()
        scroll.hasHorizontalScroller = true
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.scrollerStyle = .overlay
        scroll.drawsBackground = false
        scroll.contentView.drawsBackground = false
        scroll.automaticallyAdjustsContentInsets = false
        let host = NSHostingView(rootView: AnyView(content()))
        host.sizingOptions = []
        scroll.documentView = host
        context.coordinator.host = host
        context.coordinator.controller = controller
        let pan = NSPanGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.panned(_:)))
        pan.buttonMask = 1
        pan.delaysPrimaryMouseButtonEvents = false
        pan.delegate = context.coordinator
        scroll.addGestureRecognizer(pan)
        controller.attach(to: scroll)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        let coordinator = context.coordinator
        coordinator.controller = controller
        controller.attach(to: scroll)
        guard let host = coordinator.host else { return }
        let size = CGSize(width: max(scroll.contentSize.width, documentSize.width),
                          height: max(scroll.contentSize.height, documentSize.height))
        host.rootView = AnyView(content().frame(width: size.width, height: size.height, alignment: .topLeading))
        host.setFrameSize(size)
        scroll.layoutSubtreeIfNeeded()
        if coordinator.pageID != pageID {
            controller.endDrag()
            scroll.contentView.scroll(to: .zero)
            coordinator.pageID = pageID
        } else {
            let clip = scroll.contentView
            clip.scroll(to: clip.constrainBoundsRect(clip.bounds).origin)
        }
        scroll.reflectScrolledClipView(scroll.contentView)
    }

    static func dismantleNSView(_ scroll: NSScrollView, coordinator: Coordinator) {
        coordinator.controller?.detach(from: scroll)
    }

    @MainActor final class Coordinator: NSObject, NSGestureRecognizerDelegate {
        var host: NSHostingView<AnyView>?
        weak var controller: ImagePanController?
        var pageID: URL?

        @objc func panned(_ recognizer: NSPanGestureRecognizer) {
            guard let view = recognizer.view else { return }
            switch recognizer.state {
            case .began, .changed, .ended:
                let delta = recognizer.translation(in: view)
                controller?.drag(translation: CGSize(width: delta.x,
                    height: view.isFlipped ? delta.y : -delta.y))
                if recognizer.state == .ended { controller?.endDrag() }
            case .cancelled, .failed:
                controller?.endDrag()
            default: break
            }
        }

        func gestureRecognizer(_ gestureRecognizer: NSGestureRecognizer,
                               shouldRecognizeSimultaneouslyWith otherGestureRecognizer: NSGestureRecognizer) -> Bool { true }
    }

    private final class ImageScrollView: NSScrollView {
        override func resetCursorRects() {
            super.resetCursorRects()
            addCursorRect(contentView.frame, cursor: .openHand)
        }
    }
}

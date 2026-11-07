import AppKit

private final class FlippedDocument: NSView {
    override var isFlipped: Bool { true }
}

@main struct ImagePanTests {
    @MainActor static func main() {
        NSApplication.shared.setActivationPolicy(.prohibited)
        let controller = ImagePanController()
        let first = makeScroll(flipped: true)
        let clip = first.contentView
        clip.scroll(to: NSPoint(x: 300, y: 250))
        controller.attach(to: first)
        controller.drag(translation: CGSize(width: -100, height: -80))
        expect(clip.bounds.origin, NSPoint(x: 400, y: 330))
        controller.drag(translation: CGSize(width: -140, height: -100))
        expect(clip.bounds.origin, NSPoint(x: 440, y: 350))
        controller.endDrag()

        // Wheel/trackpad changes between mouse drags become the next anchor.
        clip.scroll(to: NSPoint(x: 500, y: 400))
        controller.attach(to: first)
        controller.drag(translation: CGSize(width: 60, height: 50))
        expect(clip.bounds.origin, NSPoint(x: 440, y: 350))
        controller.endDrag()
        controller.drag(translation: CGSize(width: 10_000, height: 10_000))
        expect(clip.bounds.origin, .zero)
        controller.endDrag()
        controller.drag(translation: CGSize(width: -10_000, height: -10_000))
        expect(clip.bounds.origin, NSPoint(x: 1000 - clip.bounds.width, y: 800 - clip.bounds.height))
        controller.endDrag()

        let second = makeScroll(flipped: true)
        second.contentView.scroll(to: NSPoint(x: 100, y: 100))
        controller.attach(to: second)
        controller.detach(from: first) // A previous page can dismantle late.
        controller.drag(translation: CGSize(width: -30, height: -20))
        expect(second.contentView.bounds.origin, NSPoint(x: 130, y: 120))
        controller.detach(from: second)
        controller.drag(translation: CGSize(width: -100, height: -100))
        expect(second.contentView.bounds.origin, NSPoint(x: 130, y: 120))

        let unflipped = makeScroll(flipped: false)
        unflipped.contentView.scroll(to: NSPoint(x: 300, y: 250))
        controller.attach(to: unflipped)
        controller.drag(translation: CGSize(width: -100, height: 80))
        expect(unflipped.contentView.bounds.origin, NSPoint(x: 400, y: 330))
        controller.endDrag()
        let fitted = NSScrollView(frame: NSRect(x: 0, y: 0, width: 200, height: 150))
        fitted.documentView = FlippedDocument(frame: NSRect(x: 0, y: 0, width: 100, height: 80))
        controller.attach(to: fitted)
        controller.drag(translation: CGSize(width: -400, height: -300))
        expect(fitted.contentView.bounds.origin, .zero)
        controller.endDrag()
        print("Native image pan checks passed: both axes, cumulative movement, bounds, wheel anchor, page replacement, and coordinate direction")
    }

    @MainActor private static func makeScroll(flipped: Bool) -> NSScrollView {
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 200, height: 150))
        scroll.hasHorizontalScroller = false
        scroll.hasVerticalScroller = false
        scroll.documentView = flipped
            ? FlippedDocument(frame: NSRect(x: 0, y: 0, width: 1000, height: 800))
            : NSView(frame: NSRect(x: 0, y: 0, width: 1000, height: 800))
        return scroll
    }

    private static func expect(_ value: NSPoint, _ expected: NSPoint) {
        precondition(abs(value.x - expected.x) < 0.1 && abs(value.y - expected.y) < 0.1,
                     "Expected \(expected), received \(value)")
    }
}

import AppKit

struct ReaderBlockMarker: Identifiable {
    let block: TextBlock
    let number: Int
    let frame: CGRect
    let badge: CGRect?
    var id: String { block.id }

    static let font = NSFont.monospacedDigitSystemFont(ofSize: 10, weight: .semibold)
    private static let digitWidth = ("0" as NSString).size(withAttributes: [.font: font]).width

    static func layout(page: PageResult, size: CGSize, boxMode: Bool) -> [Self] {
        var occupied: [CGRect] = []
        return page.blocks.enumerated().compactMap { index, block in
            let marked = !block.keepsOriginal && (boxMode || !block.error.isEmpty)
            guard marked || block.keepsOriginal,
                  let frame = ReaderCanvasGeometry.blockFrame(block, page: page, size: size) else { return nil }
            let number = index + 1
            var badge: CGRect?
            if marked {
                // Measure the number in display points, independently of the OCR column and zoom.
                let width = ceil(digitWidth * CGFloat(String(number).count)) + 8
                let badgeSize = CGSize(width: min(size.width, width), height: min(size.height, 16))
                let placed = placeBadge(for: frame, size: badgeSize, page: size, occupied: occupied)
                badge = placed
                occupied.append(placed)
            }
            return Self(block: block, number: number, frame: frame, badge: badge)
        }
    }

    private static func placeBadge(for box: CGRect, size: CGSize, page: CGSize, occupied: [CGRect]) -> CGRect {
        let gap: CGFloat = 2
        let maxX = page.width - size.width, maxY = page.height - size.height
        let anchor = min(maxX, max(0, box.minX))
        let above = box.minY - gap - size.height, below = box.maxY + gap
        var rows = [above, below].filter { (0...maxY).contains($0) }
        if rows.isEmpty { rows = [min(maxY, max(0, above))] }

        func badge(_ x: CGFloat, _ y: CGFloat) -> CGRect {
            CGRect(origin: CGPoint(x: x, y: y), size: size)
        }

        // Try the two box edges before shifting along either row.
        for y in rows {
            let candidate = badge(anchor, y)
            if occupied.allSatisfy({ !$0.intersects(candidate) }) { return candidate }
        }

        let byX = occupied.sorted { $0.minX < $1.minX }
        func freeSpot(on y: CGFloat, toRight: Bool) -> CGRect? {
            var candidate = badge(anchor, y)
            for i in byX.indices {
                let neighbor = byX[toRight ? i : byX.count - 1 - i]
                if neighbor.intersects(candidate) {
                    candidate.origin.x = toRight ? neighbor.maxX + gap : neighbor.minX - gap - size.width
                    if !(0...maxX).contains(candidate.minX) { return nil }
                }
            }
            return candidate
        }
        for toRight in [true, false] {
            for y in rows.reversed() {
                if let candidate = freeSpot(on: y, toRight: toRight) { return candidate }
            }
        }

        // Both rows can be packed at a page edge. Check the nearest remaining clear row.
        let extraRows = Set([0] + occupied.map { $0.maxY + gap }).filter { $0 <= maxY }
        for y in extraRows.sorted(by: { (abs($0 - rows[0]), $0) < (abs($1 - rows[0]), $1) }) {
            if let candidate = freeSpot(on: y, toRight: true) ?? freeSpot(on: y, toRight: false) { return candidate }
        }
        return badge(anchor, rows[0])
    }
}

import SwiftUI
import AppKit

private typealias ControlState<Value> = SwiftUI.State<Value>

enum ReaderPalette {
    private static func adaptive(_ light: UInt32, _ dark: UInt32) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            let value = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? dark : light
            return NSColor(srgbRed: CGFloat((value >> 16) & 255) / 255,
                           green: CGFloat((value >> 8) & 255) / 255,
                           blue: CGFloat(value & 255) / 255, alpha: 1)
        })
    }

    static let bar = adaptive(0xfaf8f4, 0x282624)
    static let inspector = adaptive(0xf7f5ef, 0x262421)
    static let canvas = adaptive(0xdedbd4, 0x1a1917)
    static let text = adaptive(0x2d2b28, 0xeeeae2)
    static let secondary = adaptive(0x625d55, 0xb9b3a9)
    static let separator = adaptive(0xd4cfc5, 0x49443d)
    static let fieldBorder = adaptive(0x938b7e, 0x857c70)
    static let selection = adaptive(0xe3ded4, 0x3a3530)
    static let control = adaptive(0xf0ece4, 0x38342f)
    static let controlHover = adaptive(0xe4ded4, 0x494239)
    static let accent = adaptive(0x4c4037, 0xe0d2be)
    static let primary = adaptive(0x35312c, 0xe5ddd0)
    static let onPrimary = adaptive(0xfffaf1, 0x2b2722)
    // These marks sit on the original comic, whose colors do not follow macOS appearance.
    static let marker = Color(red: 119.0 / 255, green: 83.0 / 255, blue: 61.0 / 255)
}

struct ReaderControlButtonStyle: ButtonStyle {
    var selected = false

    func makeBody(configuration: Configuration) -> some View {
        Control(configuration: configuration, selected: selected)
    }

    private struct Control: View {
        @Environment(\.isEnabled) private var enabled
        @ControlState private var hovered = false
        let configuration: ButtonStyleConfiguration
        let selected: Bool

        var body: some View {
            configuration.label
                .padding(.horizontal, 6).frame(minWidth: 28, minHeight: 28)
                .foregroundStyle(ReaderPalette.text)
                .background(configuration.isPressed || selected ? ReaderPalette.selection :
                    (hovered && enabled ? ReaderPalette.controlHover : ReaderPalette.control),
                    in: RoundedRectangle(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6)
                    .strokeBorder(ReaderPalette.fieldBorder.opacity(enabled ? 0.7 : 0.35), lineWidth: 0.5))
                .contentShape(RoundedRectangle(cornerRadius: 6))
                .opacity(enabled ? 1 : 0.4)
                .onHover { hovered = $0 }
        }
    }
}

struct ReaderPrimaryButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 12, weight: .semibold))
            .padding(.horizontal, 12).padding(.vertical, 7)
            .foregroundStyle(ReaderPalette.onPrimary)
            .background(ReaderPalette.primary.opacity(configuration.isPressed ? 0.85 : 1),
                        in: RoundedRectangle(cornerRadius: 6))
            .opacity(enabled ? 1 : 0.45)
    }
}

struct ReaderToolbarPrimaryButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 12, weight: .semibold))
            .padding(.horizontal, 14).frame(height: 34)
            .foregroundStyle(ReaderPalette.onPrimary)
            .background(ReaderPalette.primary.opacity(configuration.isPressed ? 0.85 : 1), in: Capsule())
            .opacity(enabled ? 1 : 0.45)
    }
}

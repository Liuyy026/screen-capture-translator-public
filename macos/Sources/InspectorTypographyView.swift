import AppKit
import SwiftUI

private typealias TypographyState<Value> = SwiftUI.State<Value>

@MainActor struct ReaderBlockTypographyView: View {
    @ObservedObject var reader: Reader
    let block: TextBlock
    let key: ReaderDraftKey
    let isDisabled: Bool
    @Binding var isExpanded: Bool
    @Binding var showsDetails: Bool

    var body: some View {
        let actual = reader.actualTypeset(for: block)
        let resolved = reader.resolvedTypography(for: block)
        let style = Binding(get: { reader.typographyOverride(for: block) }, set: { value in
            if reader.editCacheIdentity == key.cache {
                reader.setTypographyOverride(value, for: block)
            }
        })
        let disabled = isDisabled || block.keepsOriginal || block.needsOCRReview
        let summary = typographySummary(actual)
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                TypographyDisclosure(
                    title: "排版", summary: isExpanded ? nil : summary,
                    identifier: "dialogue-typography", isExpanded: $isExpanded
                ).font(.system(size: 12, weight: .semibold))
                if isExpanded {
                    Button("恢复章节默认") {
                        if reader.editCacheIdentity == key.cache { reader.resetTypographyOverride(for: block) }
                    }
                        .font(.caption).buttonStyle(.borderless)
                        .disabled(disabled || style.wrappedValue == ReadingTextStyle(textFrame: style.wrappedValue.textFrame))
                        .help("恢复章节默认的字体、字号、文字方向、间距与对齐")
                        .accessibilityIdentifier("dialogue-typography-reset")
                }
            }.frame(minHeight: 24)
            if isExpanded {
                TypographyFields(style: style, resolved: resolved, actual: actual, prefix: "dialogue")
                    .id(key)
                    .disabled(disabled)
                if block.needsReadingOverlay {
                    HStack {
                        Text("在中文画面拖动排字框，两侧可调宽。")
                            .font(.caption).foregroundStyle(ReaderPalette.secondary)
                        Spacer(minLength: 4)
                        if style.wrappedValue.textFrame != nil {
                            Button("恢复位置") {
                                if reader.editCacheIdentity == key.cache { reader.setTextFrame(nil, for: block) }
                            }.font(.caption).buttonStyle(.borderless).disabled(disabled)
                                .help("恢复这条对白的自动位置和宽度").accessibilityIdentifier("reset-dialogue-frame")
                        }
                    }
                }
                TypographyDisclosure(
                    title: "间距与对齐", summary: spacingSummary(actual, resolved: resolved),
                    identifier: "dialogue-typography-spacing", isExpanded: $showsDetails
                ).font(.system(size: 12))
                if showsDetails {
                    TypographySpacingFields(style: style, resolved: resolved, actual: actual, prefix: "dialogue")
                        .id(key)
                        .disabled(disabled)
                }
            }
        }
    }

    private func typographySummary(_ actual: ReadingTypeset?) -> String {
        if block.keepsOriginal { return "保留原框" }
        if block.needsOCRReview { return "待核对" }
        if let actual {
            let size = actual.sourceFontSize.formatted(.number.precision(.fractionLength(0...1)))
            return "\(TypographyFonts.displayName(actual.fontName)) · \(size) px · \(actual.direction.label)"
        }
        if block.translation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return "待翻译" }
        if block.needsReadingOverlay && reader.readingRegions == nil && reader.readingLayoutError == nil { return "待排版" }
        return "未回填"
    }

    private func spacingSummary(_ actual: ReadingTypeset?, resolved: ReadingTextStyle) -> String {
        let spacing = actual?.lineSpacing ?? resolved.lineSpacing
        let spacingLabel = spacing.map { $0.formatted(.number.precision(.fractionLength(0...2))) + " 倍" }
        let direction = actual?.direction ?? resolved.direction ?? .automatic
        let alignment = actual?.alignment ?? resolved.alignment
        return (spacingLabel ?? "自动间距") + " · " + (alignment?.label(for: direction) ?? "自动对齐")
    }
}

@MainActor struct ReaderChapterTypographyEditor: View {
    @ObservedObject var reader: Reader
    @Environment(\.dismiss) private var dismiss
    @TypographyState private var style: ReadingTextStyle
    @TypographyState private var error: String?

    init(reader: Reader) {
        self.reader = reader
        _style = TypographyState(initialValue: reader.chapterTypography)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("章节排版默认").font(.headline)
            Text("本章中采用章节默认的对白会使用这些设置。")
                .font(.caption).foregroundStyle(ReaderPalette.secondary)
            TypographyFields(style: $style, resolved: style, actual: nil, prefix: "chapter")
            Divider().overlay(ReaderPalette.separator)
            TypographySpacingFields(style: $style, resolved: style, actual: nil, prefix: "chapter")
            if let error {
                Text(error).font(.caption).foregroundStyle(.orange).textSelection(.enabled)
            }
            HStack(spacing: 8) {
                Button("恢复自动排版") { style = ReadingTextStyle() }
                    .buttonStyle(.borderless).disabled(style == ReadingTextStyle())
                    .accessibilityIdentifier("chapter-typography-reset")
                Spacer(minLength: 8)
                Button("取消") { dismiss() }
                    .buttonStyle(ReaderControlButtonStyle()).keyboardShortcut(.cancelAction)
                Button("应用", action: apply)
                    .buttonStyle(ReaderPrimaryButtonStyle()).keyboardShortcut(.defaultAction)
                    .accessibilityIdentifier("chapter-typography-confirm")
            }
        }.padding(20).frame(width: 360)
            .background(ReaderPalette.bar).foregroundStyle(ReaderPalette.text).tint(ReaderPalette.accent)
            .onChange(of: reader.files) { _, _ in dismiss() }
    }

    private func apply() {
        if reader.setChapterTypography(style) {
            dismiss()
        } else {
            error = reader.error ?? "未能保存章节排版默认。"
        }
    }
}

private struct TypographyDisclosure: View {
    let title: String
    let summary: String?
    let identifier: String
    @Binding var isExpanded: Bool

    var body: some View {
        Button { isExpanded.toggle() } label: {
            HStack(spacing: 7) {
                Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                    .font(.system(size: 9, weight: .semibold))
                    .frame(width: 9).accessibilityHidden(true)
                Text(title).fixedSize()
                Spacer(minLength: 5)
                if let summary {
                    Text(summary).font(.caption).foregroundStyle(ReaderPalette.secondary)
                        .lineLimit(1).truncationMode(.head)
                }
            }.padding(.vertical, 4).contentShape(Rectangle())
        }.buttonStyle(.plain)
            .accessibilityLabel(title)
            .accessibilityValue((isExpanded ? "已展开" : "已收起") + (summary.map { "，" + $0 } ?? ""))
            .accessibilityIdentifier(identifier)
            .help(summary ?? title)
    }
}

private struct TypographyFields: View {
    @Binding var style: ReadingTextStyle
    let resolved: ReadingTextStyle
    let actual: ReadingTypeset?
    let prefix: String

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack(alignment: .top, spacing: 10) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("字体").font(.caption).foregroundStyle(ReaderPalette.secondary)
                    TypographyFontMenu(font: $style.fontName, resolved: resolved.fontName, prefix: prefix)
                }.frame(maxWidth: .infinity, alignment: .leading)
                VStack(alignment: .leading, spacing: 4) {
                    Text("字号").font(.caption).foregroundStyle(ReaderPalette.secondary)
                    TypographyNumberField(
                        value: $style.fontSize, inherited: resolved.fontSize, displayFallback: actual?.sourceFontSize,
                        baseline: 24, range: ReadingTextStyle.fontSizeRange, step: 1,
                        title: "字号", unit: "px", identifier: prefix + "-typography-size"
                    )
                }.frame(minWidth: 100)
            }
            HStack(spacing: 8) {
                Text("方向").font(.caption).foregroundStyle(ReaderPalette.secondary)
                Picker("方向", selection: Binding(get: {
                    style.direction ?? resolved.direction ?? .automatic
                }, set: { style.direction = $0 })) {
                    ForEach(ReadingTextDirection.allCases, id: \.self) { Text($0.label).tag($0) }
                }.labelsHidden().pickerStyle(.segmented)
                    .accessibilityIdentifier(prefix + "-typography-direction")
            }
        }
    }
}

private struct TypographySpacingFields: View {
    @Binding var style: ReadingTextStyle
    let resolved: ReadingTextStyle
    let actual: ReadingTypeset?
    let prefix: String

    private var direction: ReadingTextDirection { actual?.direction ?? style.direction ?? resolved.direction ?? .automatic }

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 8) {
                let title = direction == .vertical ? "列间距" : direction == .horizontal ? "行间距" : "行列间距"
                Text(title).font(.caption).foregroundStyle(ReaderPalette.secondary)
                Spacer(minLength: 5)
                TypographyNumberField(
                    value: $style.lineSpacing, inherited: resolved.lineSpacing, displayFallback: actual?.lineSpacing,
                    baseline: direction == .vertical ? 1.24 : 1.38, range: ReadingTextStyle.lineSpacingRange, step: 0.1,
                    title: title, unit: "倍", identifier: prefix + "-typography-line-spacing"
                )
            }
            HStack(spacing: 8) {
                Text("对齐").font(.caption).foregroundStyle(ReaderPalette.secondary)
                Spacer(minLength: 5)
                Picker("对齐", selection: $style.alignment) {
                    Text("默认").tag(nil as ReadingTextAlignment?)
                    ForEach(ReadingTextAlignment.allCases, id: \.self) {
                        Text($0.label(for: direction)).tag(Optional($0))
                    }
                }.labelsHidden()
                    .accessibilityIdentifier(prefix + "-typography-alignment")
            }
        }
    }
}

private struct TypographyNumberField: View {
    @Binding var value: Double?
    let inherited: Double?
    let displayFallback: Double?
    let baseline: Double
    let range: ClosedRange<Double>
    let step: Double
    let title: String
    let unit: String
    let identifier: String

    private var displayed: Double? { value ?? inherited ?? displayFallback }
    private var current: Double { displayed ?? baseline }
    private var enteredValue: Binding<Double?> {
        Binding(get: { displayed }, set: { newValue in
            guard let newValue else { value = nil; return }
            if newValue.isFinite { value = min(max(newValue, range.lowerBound), range.upperBound) }
        })
    }

    var body: some View {
        HStack(spacing: 0) {
            stepButton("minus", change: -step, suffix: "decrease", label: "减小" + title)
                .disabled(current <= range.lowerBound)
            HStack(spacing: 2) {
                TextField(title, value: enteredValue, format: .number.precision(.fractionLength(0...2)), prompt: Text("自动"))
                    .textFieldStyle(.plain).multilineTextAlignment(.trailing)
                    .monospacedDigit().frame(width: 40)
                    .accessibilityIdentifier(identifier)
                    .help("清空\(title)可恢复默认值")
                Text(unit).foregroundStyle(ReaderPalette.secondary)
                    .opacity(displayed == nil ? 0 : 1).accessibilityHidden(true)
            }.padding(.horizontal, 4)
            stepButton("plus", change: step, suffix: "increase", label: "增大" + title)
                .disabled(current >= range.upperBound)
        }.font(.system(size: 11))
            .background(ReaderPalette.bar, in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(ReaderPalette.fieldBorder.opacity(0.7), lineWidth: 0.5))
    }

    private func stepButton(_ icon: String, change: Double, suffix: String, label: String) -> some View {
        Button {
            enteredValue.wrappedValue = ((current + change) * 100).rounded() / 100
        } label: {
            Image(systemName: icon).font(.system(size: 10, weight: .semibold))
                .frame(width: 22, height: 24).contentShape(Rectangle())
        }.buttonStyle(.borderless)
            .accessibilityLabel(label).help(label)
            .accessibilityIdentifier(identifier + "-" + suffix)
    }
}

private struct TypographyFontMenu: View {
    @Binding var font: String?
    let resolved: String?
    let prefix: String

    var body: some View {
        let name = TypographyFonts.displayName(font ?? resolved)
        Menu {
            if prefix == "chapter" {
                fontOption("系统字体", value: nil)
            } else {
                fontOption("跟随章节", value: nil)
                fontOption("系统字体", value: "")
            }
            Divider()
            ForEach(TypographyFonts.preferred) { fontOption($0.label, value: $0.id) }
            Menu("其他字体") {
                ForEach(TypographyFonts.otherFamilies, id: \.self) { family in
                    fontOption(family, value: family)
                }
            }
        } label: {
            Text(name).lineLimit(1).frame(maxWidth: .infinity, alignment: .leading)
        }.menuStyle(.borderedButton).menuIndicator(.visible)
            .accessibilityLabel("字体").accessibilityValue(name)
            .accessibilityIdentifier(prefix + "-typography-font")
            .help(name)
    }

    private func fontOption(_ title: String, value: String?) -> some View {
        Button { font = value } label: {
            if font == value {
                Label(title, systemImage: "checkmark")
            } else {
                Text(title)
            }
        }
    }
}

@MainActor private enum TypographyFonts {
    struct Option: Identifiable {
        let id: String
        let label: String
    }

    static let preferred = [
        Option(id: "PingFangSC-Medium", label: "苹方"),
        Option(id: "STSongti-SC-Regular", label: "宋体"),
        Option(id: "STKaitiSC-Regular", label: "楷体"),
        Option(id: "HiraginoSansGB-W3", label: "冬青黑体"),
    ].filter { NSFont(name: $0.id, size: 12) != nil }

    static let otherFamilies = NSFontManager.shared.availableFontFamilies.sorted {
        $0.localizedStandardCompare($1) == .orderedAscending
    }

    static func displayName(_ name: String?) -> String {
        guard let name, !name.isEmpty else { return "系统字体" }
        return preferred.first(where: { $0.id == name })?.label ?? NSFont(name: name, size: 12)?.displayName ?? name
    }
}

private extension ReadingTextDirection {
    var label: String {
        switch self {
        case .automatic: "自动"
        case .horizontal: "横排"
        case .vertical: "竖排"
        }
    }
}

private extension ReadingTextAlignment {
    func label(for direction: ReadingTextDirection) -> String {
        switch (self, direction) {
        case (.center, _): "居中"
        case (.leading, .horizontal): "靠左"
        case (.trailing, .horizontal): "靠右"
        case (.leading, .vertical): "靠上"
        case (.trailing, .vertical): "靠下"
        case (.leading, .automatic): "对齐起端"
        case (.trailing, .automatic): "对齐末端"
        }
    }
}

import SwiftUI

struct ReaderInspectorView: View {
    @ObservedObject var reader: Reader
    @ObservedObject var workspace: ReaderWorkspaceState

    private var selectedBlock: TextBlock? {
        reader.page?.blocks.first { $0.id == reader.selectedBlock }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("对白对照").font(.system(size: 13, weight: .semibold))
                Spacer()
                Text("\(reader.page?.blocks.count ?? 0) 处文字").font(.caption)
                    .foregroundStyle(ReaderPalette.secondary)
            }.padding(12)
            Divider().overlay(ReaderPalette.separator)
            if let page = reader.page {
                if let block = selectedBlock, let cache = reader.editCacheIdentity {
                    ReaderSelectedBlockEditor(reader: reader, workspace: workspace, block: block,
                                              key: ReaderDraftKey(cache: cache, blockID: block.id))
                    Divider().overlay(ReaderPalette.separator)
                }
                Text("本页对白").font(.caption).foregroundStyle(ReaderPalette.secondary)
                    .padding(.horizontal, 12).padding(.top, 12).padding(.bottom, 8)
                if page.blocks.isEmpty {
                    Text("没有检测到文字。复杂背景、手写字或低清晰度图片可能漏检。")
                        .font(.callout).foregroundStyle(ReaderPalette.secondary).padding(12)
                } else {
                    ScrollViewReader { proxy in
                        ScrollView {
                            LazyVStack(spacing: 2) {
                                ForEach(page.blocks) { block in
                                    dialogueRow(block).id(block.id)
                                }
                            }.padding(.horizontal, 6).padding(.bottom, 8)
                        }
                        .onChange(of: reader.selectedBlock) { _, id in
                            if let id { proxy.scrollTo(id, anchor: .center) }
                        }
                        .accessibilityElement(children: .contain)
                        .accessibilityIdentifier("dialogue-list")
                    }.frame(minHeight: 100)
                }
            } else {
                VStack(alignment: .leading, spacing: 12) {
                    Image(systemName: "character.bubble").font(.system(size: 28))
                    Text("识别后，日文和中文会出现在这里。")
                    Text("点击图上的文字框定位对白，再修正原文或译文。")
                        .font(.caption)
                }.foregroundStyle(ReaderPalette.secondary).padding(16)
                Spacer(minLength: 0)
            }
        }.frame(width: 250).frame(maxHeight: .infinity)
            .foregroundStyle(ReaderPalette.text).background(ReaderPalette.inspector)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("dialogue-inspector")
    }

    private func dialogueRow(_ block: TextBlock) -> some View {
        let active = reader.selectedBlock == block.id
        let source = block.source.isEmpty ? "尚无原文" : block.source
        let title = block.keepsOriginal ? source : (block.translation.isEmpty ? "尚未翻译" : block.translation)
        let detail = block.keepsOriginal ? "保留原框" : source
        return Button { reader.selectedBlock = block.id } label: {
            HStack(alignment: .top, spacing: 6) {
                Text(block.id.count == 1 ? "0" + block.id : block.id)
                    .font(.system(size: 11, weight: active ? .semibold : .regular).monospacedDigit())
                    .foregroundStyle(active ? ReaderPalette.accent : ReaderPalette.secondary)
                    .frame(width: 24, alignment: .leading)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(.system(size: 12)).lineLimit(1)
                    Text(detail)
                        .font(.system(size: 11)).foregroundStyle(ReaderPalette.secondary).lineLimit(1)
                }.frame(maxWidth: .infinity, alignment: .leading)
                if !block.keepsOriginal && !block.error.isEmpty {
                    Image(systemName: "exclamationmark.circle").foregroundStyle(.orange).font(.caption)
                }
            }.padding(.horizontal, 6).padding(.vertical, 7)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(active ? ReaderPalette.selection : .clear, in: RoundedRectangle(cornerRadius: 6))
                .contentShape(Rectangle())
        }.buttonStyle(.plain)
            .help(block.keepsOriginal || block.translation.isEmpty ? source : block.translation)
            .accessibilityLabel("对白 " + block.id + "，" + title + "，" + detail)
            .accessibilityIdentifier("dialogue-" + block.id)
    }
}

private struct ReaderSelectedBlockEditor: View {
    @ObservedObject var reader: Reader
    @ObservedObject var workspace: ReaderWorkspaceState
    let block: TextBlock
    let key: ReaderDraftKey

    private var draft: ReaderEditDraft { workspace.draft(for: key, block: block) }
    private var disabled: Bool { reader.busy || reader.preparingImage || draft.phase != .editing }
    private var index: Int { reader.page?.blocks.firstIndex(where: { $0.id == block.id }) ?? 0 }
    private var warnings: [String] {
        var values: [String] = []
        if !block.keepsOriginal {
            if !block.error.isEmpty { values.append(block.error) }
            values += (block.quality_warnings ?? []).filter { $0.hasPrefix("原文与译文相同") }
            if let note = reader.readingPreserved[block.id] { values.append(note) }
        }
        if draft.sourceChanged { values.append("修改日文后，保存会清除这条对白的旧译文。") }
        return values
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Text("对白 " + (block.id.count == 1 ? "0" + block.id : block.id))
                    .font(.system(size: 12, weight: .semibold)).foregroundStyle(ReaderPalette.accent)
                if draft.changed {
                    Text(draft.phase == .editing ? "未保存" : (draft.phase == .saving ? "保存中…" : "已保存"))
                        .font(.system(size: 11)).foregroundStyle(ReaderPalette.secondary)
                } else if block.edited {
                    Text("已修改").font(.system(size: 11)).foregroundStyle(ReaderPalette.secondary)
                }
                Spacer(minLength: 0)
                Button { move(-1) } label: { Image(systemName: "chevron.up") }
                    .disabled(index == 0).help("上一条对白").accessibilityIdentifier("previous-dialogue")
                Button { move(1) } label: { Image(systemName: "chevron.down") }
                    .disabled(index >= (reader.page?.blocks.count ?? 0) - 1)
                    .help("下一条对白").accessibilityIdentifier("next-dialogue")
            }.buttonStyle(ReaderControlButtonStyle()).frame(height: 28)
            Picker("对话框处理", selection: Binding(get: { block.keepsOriginal }, set: {
                reader.setPreserveBubble($0, id: block.id)
            })) {
                Text("翻译此框").tag(false)
                Text("保留原框").tag(true)
            }.labelsHidden().pickerStyle(.segmented)
                .disabled(disabled)
                .help("保留原框会对整个对话框跳过翻译和回填。")
                .accessibilityIdentifier("dialogue-language")
            field("日文原文", text: Binding(get: { draft.source }, set: {
                workspace.update(key, block: block, source: $0)
            }), height: 76, identifier: "dialogue-source")
            field("中文译文", text: Binding(get: {
                draft.phase == .saved ? draft.savedTranslation : draft.translation
            }, set: {
                workspace.update(key, block: block, translation: $0)
            }), height: 64, identifier: "dialogue-translation")
            if let suggestion = block.vision_source, !suggestion.isEmpty, suggestion != block.source {
                ScrollView {
                    VStack(alignment: .leading, spacing: 6) {
                        Text("画面复核建议：" + suggestion).font(.caption).textSelection(.enabled)
                        Button("填入日文，核对后保存") { workspace.update(key, block: block, source: suggestion) }
                            .font(.caption).disabled(disabled)
                    }
                }.frame(maxHeight: 72).foregroundStyle(ReaderPalette.secondary)
            }
            if !warnings.isEmpty {
                ScrollView {
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(warnings, id: \.self) { Text($0).font(.caption).textSelection(.enabled) }
                    }.frame(maxWidth: .infinity, alignment: .leading)
                }.frame(maxHeight: 64).foregroundStyle(ReaderPalette.secondary)
            }
            HStack {
                Button { reader.retryBlock(block.id) } label: { Label("重新翻译", systemImage: "arrow.clockwise") }
                    .buttonStyle(.borderless).font(.system(size: 12))
                    .disabled(disabled || draft.changed || !reader.engineReady || block.keepsOriginal)
                    .accessibilityIdentifier("retry-dialogue")
                Spacer(minLength: 4)
                Button("保存修改", action: save).buttonStyle(ReaderPrimaryButtonStyle())
                    .disabled(disabled || !draft.changed).accessibilityIdentifier("save-dialogue")
            }.padding(.top, 2)
        }.padding(12)
    }

    private func field(_ title: String, text: Binding<String>, height: CGFloat, identifier: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.system(size: 11)).foregroundStyle(ReaderPalette.secondary)
            TextEditor(text: text).font(.system(size: 13)).scrollContentBackground(.hidden)
                .padding(5).frame(height: height).background(ReaderPalette.bar)
                .clipShape(RoundedRectangle(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(ReaderPalette.fieldBorder, lineWidth: 1))
                .disabled(disabled).accessibilityLabel(title).accessibilityIdentifier(identifier)
        }
    }

    private func move(_ offset: Int) {
        guard let blocks = reader.page?.blocks, blocks.indices.contains(index + offset) else { return }
        reader.selectedBlock = blocks[index + offset].id
    }

    private func save() {
        let value = draft
        guard !disabled, value.changed else { return }
        workspace.beginSave(key)
        reader.saveEdit(id: block.id, source: value.source, translation: value.translation) { succeeded in
            workspace.finishSave(key, succeeded: succeeded)
            if succeeded, let page = reader.page, reader.editCacheIdentity == key.cache {
                workspace.reconcile(cache: key.cache, page: page)
            }
        }
    }
}

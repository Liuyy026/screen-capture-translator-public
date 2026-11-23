import SwiftUI

private typealias InspectorState<Value> = SwiftUI.State<Value>

struct ReaderInspectorView: View {
    @ObservedObject var reader: Reader
    @ObservedObject var workspace: ReaderWorkspaceState
    @InspectorState private var showsTypography = false
    @InspectorState private var showsTypographyDetails = false
    @InspectorState private var editorHeight: CGFloat = 0

    private var selectedBlock: TextBlock? {
        reader.page?.blocks.first { $0.id == reader.selectedBlock }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if selectedBlock == nil || reader.editCacheIdentity == nil {
                HStack {
                    Text("对白对照").font(.system(size: 13, weight: .semibold))
                    Spacer()
                    Text("共 \(reader.page?.blocks.count ?? 0) 项").font(.caption)
                        .foregroundStyle(ReaderPalette.secondary)
                    ReaderCropOCRButton(reader: reader)
                }.padding(12)
                Divider().overlay(ReaderPalette.separator)
            }
            if let page = reader.page {
                GeometryReader { geometry in
                    VStack(alignment: .leading, spacing: 0) {
                        if let block = selectedBlock, let cache = reader.editCacheIdentity {
                            let key = ReaderDraftKey(cache: cache, blockID: block.id)
                            // Keep room for the page actions and list when the editor contents grow.
                            let available = max(0, geometry.size.height - 138)
                            ScrollView {
                                ReaderSelectedBlockEditor(reader: reader, workspace: workspace, block: block, key: key,
                                                          showsTypography: $showsTypography,
                                                          showsTypographyDetails: $showsTypographyDetails)
                                    .id(key)
                                    .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { editorHeight = $0 }
                            }.scrollBounceBehavior(.basedOnSize)
                                .frame(height: min(editorHeight > 0 ? editorHeight : available, available))
                            Divider().overlay(ReaderPalette.separator)
                        }
                        pageHeading(page)
                        if page.blocks.isEmpty {
                            Text("没有检测到文字。复杂背景、手写字或低清晰度图片可能漏检。")
                                .font(.callout).foregroundStyle(ReaderPalette.secondary).padding(12)
                        } else {
                            dialogueList(page)
                        }
                    }
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
        }.frame(maxWidth: .infinity, maxHeight: .infinity)
            .foregroundStyle(ReaderPalette.text).background(ReaderPalette.inspector)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("dialogue-inspector")
    }

    private func pageHeading(_ page: PageResult) -> some View {
        let pendingReview = page.blocks.filter(\.needsOCRReview).count
        return HStack {
            Text("本页对白").font(.system(size: 12, weight: .semibold))
            if pendingReview > 0 {
                Text("\(pendingReview) 处待核对").font(.caption).foregroundStyle(ReaderPalette.secondary)
                    .help("已识别 \(page.blocks.count - pendingReview) 处文字，另有 \(pendingReview) 处待核对")
            }
            Spacer(minLength: 8)
            Button { reader.reflowCurrentPageTypography() } label: {
                Label("重排本页", systemImage: "arrow.clockwise")
            }.buttonStyle(.borderless).font(.system(size: 12))
                .disabled(page.blocks.isEmpty || reader.readingRegions == nil || reader.busy || reader.openingFolder
                    || reader.preparingImage || reader.loadingPage)
                .help("用现有译文和排版设置重新排版本页")
                .accessibilityIdentifier("reflow-page-typography")
        }.padding(.horizontal, 12).frame(height: 36)
    }

    private func dialogueList(_ page: PageResult) -> some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 2) {
                    ForEach(Array(page.blocks.enumerated()), id: \.element.id) { index, block in
                        dialogueRow(block, number: index + 1).id(block.id)
                    }
                }.padding(.horizontal, 6).padding(.bottom, 8)
            }
            .onChange(of: reader.selectedBlock, initial: true) { _, id in
                if let id { proxy.scrollTo(id, anchor: .center) }
            }
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { _ in
                if let id = reader.selectedBlock { proxy.scrollTo(id, anchor: .center) }
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("dialogue-list")
        }.frame(minHeight: 100)
    }

    private func dialogueRow(_ block: TextBlock, number: Int) -> some View {
        let active = reader.selectedBlock == block.id
        let numberLabel = String(format: "%02d", number)
        let source = block.source.isEmpty ? "尚无原文" : block.source
        let title = block.needsOCRReview ? "疑似非文字" : (block.keepsOriginal ? source : (block.translation.isEmpty ? "尚未翻译" : block.translation))
        let detail = block.needsOCRReview ? "已暂停翻译与回填" : (block.keepsOriginal ? "保留原框" : source)
        let hint = block.needsOCRReview ? (block.ocr_review?.reason ?? "请核对原图") : (block.keepsOriginal || block.translation.isEmpty ? source : block.translation)
        return Button { reader.selectedBlock = block.id } label: {
            HStack(alignment: .top, spacing: 6) {
                Text(numberLabel)
                    .font(.system(size: 11, weight: active ? .semibold : .regular).monospacedDigit())
                    .foregroundStyle(active ? ReaderPalette.accent : ReaderPalette.secondary)
                    .fixedSize().frame(minWidth: 24, alignment: .leading)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(.system(size: 12)).lineLimit(1)
                    Text(detail)
                        .font(.system(size: 11)).foregroundStyle(ReaderPalette.secondary).lineLimit(1)
                }.frame(maxWidth: .infinity, alignment: .leading)
                if !block.keepsOriginal && block.translationIssue != nil {
                    Image(systemName: "exclamationmark.circle").foregroundStyle(.orange).font(.caption)
                }
            }.padding(.horizontal, 6).padding(.vertical, 7)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(active ? ReaderPalette.selection : .clear, in: RoundedRectangle(cornerRadius: 6))
                .contentShape(Rectangle())
        }.buttonStyle(.plain)
            .help(hint + "\n文字框编号 " + block.id)
            .accessibilityLabel("对白 " + numberLabel + "，" + title + "，" + detail)
            .accessibilityIdentifier("dialogue-" + block.id)
    }
}

private struct ReaderCropOCRButton: View {
    @ObservedObject var reader: Reader

    var body: some View {
        Button { reader.beginCrop() } label: {
            Label("圈选补识别", systemImage: "viewfinder").labelStyle(.iconOnly)
        }.buttonStyle(ReaderControlButtonStyle()).disabled(!reader.canBeginCrop)
            .help("圈选补识别").accessibilityIdentifier("start-crop-ocr")
    }
}

private struct ReaderSelectedBlockEditor: View {
    @ObservedObject var reader: Reader
    @ObservedObject var workspace: ReaderWorkspaceState
    let block: TextBlock
    let key: ReaderDraftKey
    @Binding var showsTypography: Bool
    @Binding var showsTypographyDetails: Bool

    private var draft: ReaderEditDraft { workspace.draft(for: key, block: block) }
    private var disabled: Bool { reader.cropSession != nil || reader.busy || reader.openingFolder || reader.preparingImage || draft.phase != .editing }
    private var retryDisabled: Bool {
        disabled || draft.changed || !reader.engineReady || block.keepsOriginal || block.needsOCRReview
    }
    private var index: Int { reader.page?.blocks.firstIndex(where: { $0.id == block.id }) ?? 0 }
    private var warnings: [String] {
        var values: [String] = []
        if !block.keepsOriginal {
            if let issue = block.translationIssue { values.append(issue) }
            values += (block.quality_warnings ?? []).filter { $0.hasPrefix("原文与译文相同") }
            if let note = reader.readingPreserved[block.id], block.source_translation_pending != true { values.append(note) }
            else if block.needsOCRReview, let reason = block.ocr_review?.reason { values.append(reason) }
            else if block.needsReadingOverlay, let page = reader.page, let regions = reader.readingRegions,
                    !reader.readingPresentations.contains(where: { $0.region.block_ids.contains(block.id) }) {
                values.append(ReadingLayout.failureReason(blockID: block.id, page: page, regions: regions))
            }
        }
        if draft.pageChangedWhileEditing { values.append("编辑期间页面内容已更新，请核对原文和草稿后保存。") }
        if draft.clearsPreviousTranslation { values.append("只修改日文会清除旧译文；同时填写的新译文会一起保存。") }
        return values
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Text("对白 " + String(format: "%02d", index + 1))
                    .font(.system(size: 12, weight: .semibold)).foregroundStyle(ReaderPalette.accent)
                    .help("文字框编号 " + block.id)
                Text("/ \(reader.page?.blocks.count ?? 0)")
                    .font(.system(size: 12)).foregroundStyle(ReaderPalette.secondary)
                if draft.changed {
                    Text(draft.phase == .editing ? "未保存" : (draft.phase == .saving ? "保存中…" : "已保存"))
                        .font(.system(size: 11)).foregroundStyle(ReaderPalette.secondary)
                } else if block.edited {
                    Text("已修改").font(.system(size: 11)).foregroundStyle(ReaderPalette.secondary)
                }
                Spacer(minLength: 0)
                ReaderCropOCRButton(reader: reader)
                Button { move(-1) } label: { Label("上一条对白", systemImage: "chevron.up").labelStyle(.iconOnly) }
                    .disabled(index == 0).help("上一条对白").accessibilityIdentifier("previous-dialogue")
                Button { move(1) } label: { Label("下一条对白", systemImage: "chevron.down").labelStyle(.iconOnly) }
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
            }), identifier: "dialogue-source")
            field(block.needsOCRReview ? "旧译文（未采用）" : "中文译文", text: Binding(get: {
                draft.phase == .saved ? draft.savedTranslation : draft.translation
            }, set: {
                workspace.update(key, block: block, translation: $0)
            }), identifier: "dialogue-translation")
            Divider().overlay(ReaderPalette.separator)
            ReaderBlockTypographyView(reader: reader, block: block, key: key, isDisabled: disabled,
                                      isExpanded: $showsTypography, showsDetails: $showsTypographyDetails)
            if block.source_translation_pending == true, !block.translation.isEmpty {
                Button("已核对，采用现有译文") {
                    reader.saveEdit(id: block.id, source: block.source, translation: block.translation)
                }.buttonStyle(.link).font(.caption).disabled(disabled || draft.changed)
            }
            if block.needsOCRReview {
                Button("确认为文字，恢复处理") { reader.confirmOCR(block.id) }
                    .buttonStyle(.link).font(.caption).disabled(disabled || draft.changed)
                    .accessibilityIdentifier("confirm-ocr-candidate")
            }
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
                }.frame(height: 64).foregroundStyle(ReaderPalette.secondary)
            }
            HStack {
                Button { reader.retryBlock(block.id) } label: { Label("重新翻译", systemImage: "arrow.clockwise") }
                    .buttonStyle(.borderless).font(.system(size: 12))
                    .disabled(retryDisabled).opacity(retryDisabled ? 0.4 : 1)
                    .accessibilityIdentifier("retry-dialogue")
                Spacer(minLength: 4)
                if draft.changed && draft.phase != .saved {
                    Button("保存修改", action: save).buttonStyle(ReaderPrimaryButtonStyle())
                        .disabled(disabled).accessibilityIdentifier("save-dialogue")
                }
            }.frame(minHeight: 30)
        }.padding(12)
    }

    private func field(_ title: String, text: Binding<String>, identifier: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.system(size: 11)).foregroundStyle(ReaderPalette.secondary)
            // Measure at the editor's wrapping width, including the empty line after a final Return.
            Text(verbatim: text.wrappedValue + "\u{200B}")
                .lineLimit(4).fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, minHeight: 18, alignment: .leading)
                .padding(.horizontal, 8).hidden().accessibilityHidden(true)
                .overlay {
                    TextEditor(text: text).scrollContentBackground(.hidden)
                        .padding(.horizontal, 3)
                        .disabled(disabled).accessibilityLabel(title).accessibilityIdentifier(identifier)
                }
                .font(.system(size: 13)).padding(.vertical, 7)
                .background(ReaderPalette.bar)
                .clipShape(RoundedRectangle(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(ReaderPalette.fieldBorder, lineWidth: 1))
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
            workspace.finishSave(key, succeeded: succeeded, visibleCache: reader.editCacheIdentity)
            if succeeded, let page = reader.page, reader.editCacheIdentity == key.cache {
                workspace.reconcile(cache: key.cache, page: page)
            }
        }
    }
}

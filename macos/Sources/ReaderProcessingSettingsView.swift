import SwiftUI

struct ReaderProcessingSettingsView: View {
    @ObservedObject var reader: Reader
    let openConnection: () -> Void

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                Text("处理设置").font(.headline)
                Group {
                    Picker("前置增强", selection: Binding(get: { reader.srEngine }, set: {
                        reader.srEngine = $0; reader.changeEnhancement()
                    })) {
                        Text("关闭（原图）").tag("off")
                        Text("MetalFX").tag("metalfx")
                        Text("Real-CUGAN").tag("realcugan")
                    }.accessibilityIdentifier("enhancement-engine")
                    if reader.srEngine != "off" {
                        Picker("目标尺寸", selection: Binding(get: { reader.srTarget }, set: {
                            reader.srTarget = $0; reader.changeEnhancement()
                        })) {
                            ForEach(Reader.enhancementTargets, id: \.self) { target in
                                Text(verbatim: "\(target) px").tag(target)
                            }
                        }
                        Picker("按", selection: Binding(get: { reader.srAxis }, set: {
                            reader.srAxis = $0; reader.changeEnhancement()
                        })) { Text("长边").tag("long_edge"); Text("高度").tag("height") }
                        Picker("增强预设", selection: Binding(get: { reader.srPreset }, set: {
                            reader.srPreset = $0; reader.changeEnhancement()
                        })) {
                            Text("标准").tag("standard")
                            if reader.srEngine == "realcugan" {
                                Text("保守").tag("conservative"); Text("降噪").tag("denoise")
                            }
                        }
                        Text("已达目标的页面直接使用原图。切换增强会复用同一页的识别、译文和手工修改。")
                            .font(.caption).foregroundStyle(ReaderPalette.secondary)
                    }
                    if !reader.srStatus.isEmpty {
                        Text(reader.srStatus).font(.caption).foregroundStyle(ReaderPalette.secondary).textSelection(.enabled)
                    }
                }.disabled(reader.busy)
                Divider()
                Picker("翻译方式", selection: Binding(get: { reader.processingMode }, set: reader.chooseProcessingMode)) {
                    Text("普通翻译").tag("text"); Text("视觉辅助").tag("vision")
                }.pickerStyle(.segmented).disabled(reader.busy || reader.connection.provider != "omlx")
                    .accessibilityIdentifier("translation-mode")
                if reader.processingMode == "vision" {
                    Text("结合整页画面与 OCR 翻译，需要支持图片的 oMLX 模型。视觉结果单独保存，日文修正需核对后采用。")
                        .font(.caption).foregroundStyle(ReaderPalette.secondary)
                }
                HStack {
                    Text("翻译服务：" + reader.connection.name).font(.callout)
                    Spacer()
                    Button("接口设置…", action: openConnection).disabled(reader.busy)
                }
                Picker("翻译模型", selection: Binding(get: { reader.model }, set: reader.chooseModel)) {
                    if reader.availableModels.isEmpty { Text("请连接服务并获取模型").tag("") }
                    ForEach(reader.availableModels, id: \.self) { Text($0).tag($0) }
                }.disabled(reader.busy || !reader.engineReady)
                HStack {
                    Label(reader.engineReady ? "已连接" : "未连接", systemImage: reader.engineReady ? "checkmark.circle" : "exclamationmark.circle")
                        .font(.caption).foregroundStyle(ReaderPalette.secondary)
                    Spacer()
                    Button("刷新模型") { reader.startEngine() }.disabled(reader.busy)
                    Button("释放当前模型") { reader.releaseCurrentModel() }
                        .disabled(reader.busy || reader.releasingModel || reader.model.isEmpty)
                        .accessibilityIdentifier("release-translation-model")
                    if reader.releasingModel { ProgressView().controlSize(.small) }
                }
                if let notice = reader.modelReleaseNotice {
                    Text(notice).font(.caption).foregroundStyle(ReaderPalette.secondary)
                }
                if let failure = reader.modelReleaseError {
                    Text(failure).font(.caption).foregroundStyle(.orange).textSelection(.enabled)
                    Button("重试释放") { reader.retryModelRelease() }.disabled(reader.busy || reader.releasingModel)
                }
                if let error = reader.connectionError {
                    Text(error).font(.caption).foregroundStyle(.orange).textSelection(.enabled)
                }
                Divider()
                Text("图片缓存 5 GB，识别与译文缓存另限 500 MB；30 天未使用自动清理。原图、手改和来源未确认的旧缓存保留。")
                    .font(.caption).foregroundStyle(ReaderPalette.secondary)
            }.padding(16)
        }.frame(width: 330).frame(maxHeight: 610)
            .background(ReaderPalette.bar).foregroundStyle(ReaderPalette.text).tint(ReaderPalette.accent)
    }
}

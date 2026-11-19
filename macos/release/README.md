# Koma v0.21 · macOS 使用说明

适用于搭载 Apple 芯片的 Mac，部署目标为 macOS 14.0。解压下载的 ZIP，打开 `Koma.app`，点击「先试试内置测试页」或选择自己的漫画文件夹。阅读原图和使用 MetalFX 超分可以直接开始。

## 准备识别、修补与翻译

1. 退出 Koma，保持 `准备组件.command` 与 `Koma.app` 在同一目录，双击命令。首次会联网下载专用 Python、依赖、约 700 MB 的日文识别与背景修补权重，以及 Real-CUGAN 引擎和模型。完成后可将 `Koma.app` 拖到「应用程序」。
2. 如只需要其中一组组件，可在解压目录执行 `zsh ./准备组件.command ocr`（识别和背景修补）或 `zsh ./准备组件.command realcugan`（超分）。
3. 要翻译对白，请在「翻译接口设置」中填写自己运行的 Ollama 或 oMLX 地址，并选择该服务已加载或已安装的模型。文字翻译会把识别出的对白发给该地址，使用视觉辅助时也会发送页面图像。

组件和页面缓存位于 `~/Library/Application Support/KomaReaderRelease/`。勾选记住 API 密钥时，应用会将其明文保存在此目录的 `config.json`，文件仅当前用户可读写。章节的排版设置保存在漫画文件夹的 `.koma-typesetting.json`。

## 首次打开与下载校验

本版采用临时签名，未经 Apple 公证。在下载目录执行 `shasum -a 256 Koma-0.21-macOS-arm64.zip`，将输出与发布页列出的 SHA-256 对照。如果首次打开时系统提示无法验证开发者，请先确认下载来源和校验值，再按 Apple 指引操作。[在 Mac 上安全地打开 App](https://support.apple.com/zh-cn/102445)

# Koma · macOS 预发行版

适用于搭载 Apple 芯片的 Mac，部署目标为 macOS 14 或更新版本。本次在 macOS 27.0.1 实测。

1. 解压 ZIP，打开 `Koma.app`。可以直接阅读漫画并使用 MetalFX 整章超分。
2. 要使用日文识别、背景修补和 Real-CUGAN，先退出 Koma，再双击同目录的 `准备组件.command`，或在解压目录执行 `zsh ./准备组件.command`。首次需要联网，识别与修补权重约 700 MB，另有 Python 依赖和 Real-CUGAN 下载。
3. 需要翻译时，在「接口设置」中连接你自己的 Ollama 或 oMLX 服务。翻译请求会发到你配置的服务，视觉辅助会随请求发送页面图像。

组件和本版缓存位于 `~/Library/Application Support/KomaReaderRelease/`。组件准备完成后，可以把 `Koma.app` 移到「应用程序」。

本预发行版使用临时签名，Apple 公证尚待完成。核对发布页提供的 SHA-256 并确认来源后，可按 Apple 的「隐私与安全性」指引处理首次打开提示。

[Apple 的 Mac 应用打开指引](https://support.apple.com/zh-cn/102445)

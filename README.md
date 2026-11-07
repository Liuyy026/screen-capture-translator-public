# Koma · 本地漫画翻译

简体中文 | [English](README.en.md)

使用 Manga OCR 识别日文，通过本地 Ollama 和 Qwen3 翻译成简体中文。macOS 与 Windows 分目录维护，共用 Python 后端。

## 选择版本

| 版本 | 使用说明 | 启动入口 |
| --- | --- | --- |
| macOS / SwiftUI | [macOS 说明](macos/README.md) | `macos/安装.command`、`macos/启动.command` |
| Windows / PySide6 | [Windows 说明](windows/README.md) | `windows/启动Windows.bat` |

```text
macos/      macOS 源码、依赖、安装与构建
windows/    Windows 源码、依赖、启动器与界面测试
backend/    共用 OCR、翻译、缓存、结构与覆盖逻辑
scripts/    共用模型准备、标注、评估工具
tests/      共用后端测试与固定样例
samples/    原创测试图片
docs/       开发说明与评估记录
进度汇总/   历史计划与评测报告
```

请保留完整仓库。平台各自管理虚拟环境、客户端日志和构建产物；模型仍保存在仓库根目录 `.models/`。

[共享开发说明](docs/development.md) · [新旧路径对照](docs/layout-migration.md) · [历史进度](进度汇总/README.md)

[macOS 安装与翻译流程验证（2026-11-07）](docs/macos-verification-2026-11-07.md)：已在本机完成构建、启动、依赖与模型安装，以及样例 OCR 和翻译验证。

[Ollama / oMLX 接口设置与验证](docs/omlx-integration-2026-11-07.md)：两端已支持地址、API Key 和模型选择，macOS 已完成两种后端的真实样例测试。

## 功能

- 支持 WebP、PNG、JPG、JPEG、BMP、TIFF，按文件名自然排序。
- 自动识别横排和竖排文字，显示文字框、原文和中文覆盖。
- 可在右侧修改日文或中文；手动保存的中文不会被批量翻译覆盖。
- 支持单句重新翻译、整文件夹批处理、取消任务和缓存复用。
- 原图保持只读，识别结果、译文、错误状态和覆盖区域写入本地缓存。

以上编辑保存、单句重译和整文件夹批处理已有 macOS 界面入口。Windows 验证版目前支持单页处理、停止、缓存读取和结构诊断，右侧结果区仍为只读；编辑、重译和文件夹队列入口尚待接入。英文 README 仅提供英文说明，应用界面和翻译目标仍为中文。

## 当前验证结果与限制

以下验证结果为原项目保留的历史记录；本次整理未重新运行真实 OCR、GPU 或模型评测。

- macOS 原生应用已编译并本地签名；Windows 阅读器可通过启动器运行。
- Windows 基础环境此前通过 `pip check`；2026-09-19 上传前在 `.venv-cuda` 环境完成 93 项 Python 回归测试，全部通过。
- 两张自制 WebP 测试页已完成 OCR → Qwen3 4B 翻译，横排和竖排流程均通过。
- Windows RTX 5070 环境中 Manga OCR 日志显示 `Using CUDA`，Ollama 模型进程显示 GPU 使用；`samples/01.webp` 已识别出 5 个文字框，4B 和 8B 均返回 5 条有效译文。
- 输出按本次真实编号校验；占位文字、空译文、重复编号和漏译会明确标错，旧缓存和手动编辑会被保留。
- 已完成固定 62 页真实漫画的自动 A/B 与布局评估，但尚未完成足量独立标注和人工语义验收。竖排、手写、拟声词、低清图片及跨分镜阅读顺序仍可能出错，已知第 16、24 页存在结构问题。
- 中文模式是白底覆盖，不是无痕擦字或专业汉化排版；暂不支持补框/拆框、跨页人物记忆、术语表、导出汉化图片、ZIP/PDF 和子文件夹递归。
- 本地模型可能拒译、误译或返回格式错误；程序保留原文和错误信息，不保证即时或必然得到有效译文。

## 上游项目与许可证

- [Mokuro](https://github.com/kha-white/mokuro)：GPL-3.0，提供漫画检测和逐行 OCR 流程。
- [Manga OCR](https://github.com/kha-white/manga-ocr)：日文文字识别。
- [Comic Text Detector](https://github.com/dmMaze/comic-text-detector)：漫画文字检测。
- [Ollama](https://github.com/ollama/ollama)：本地模型服务。
- [Qwen3](https://github.com/QwenLM/Qwen3)：翻译模型系列。

模型和依赖不纳入 Git；各组件遵循其上游许可证。

# Koma · 本地漫画翻译

简体中文 | [English](README.en.md)

Koma 在本机识别日文漫画、翻译并在原页回填中文。`main` 中的 macOS 阅读器采用 Rust 核心和 SwiftUI 界面，Python 负责检测、OCR 和背景修补模型推理。Windows 继续使用 Python 后端。

本页说明当前源码构建。macOS 的构建产物仍名为 `Koma Rust 候选版.app`，依赖仓库目录与本机运行环境。重构前的 Python 版可在 `stable/pre-rust-20261108` 找到。

**v0.21 · 2026-11-21 · macOS 视觉辅助翻译**

第 1、2 页只发送本页图像。从第 3 页起，视觉辅助会同时发送同章上一页原图，帮助理解对话承接和人物指代，仍只翻译本页。每批关闭模型思考、强制按 JSON 结构返回结果，输出上限为 8192 token。请使用支持多图输入、关闭思考和结构化输出的 oMLX 服务及视觉模型。普通翻译继续使用原有文本流程。[视觉辅助使用说明](macos/README.md#视觉辅助翻译omlx)

**v0.21 · 2026-11-18 · macOS 阅读器**

可以为单条对白调整字体、字号和横竖排，也可以用已保存的译文重排本页，保留手改内容。拖动漫画与对白栏之间的分隔线即可调整栏宽，下次打开会恢复上次设置。本版还修复了 Display P3 彩页导出 PNG 时的底色偏移。[v0.21 发布说明](https://github.com/Liuyy026/screen-capture-translator-public/releases/tag/v0.21) · [排版操作与验证记录](docs/macos-typesetting-2026-11-18.md)

**下载 Mac 应用**　Apple Silicon、macOS 14 及以上可直接下载 ZIP，解压后打开 `Koma.app`。阅读和 MetalFX 超分可直接使用；需要日文 OCR、背景修补或 Real-CUGAN 时，先退出应用，再运行同目录的 `准备组件.command`。[下载 v0.21 Mac 应用](https://github.com/Liuyy026/screen-capture-translator-public/releases/download/v0.21/Koma-0.21-macOS-arm64.zip) · [SHA-256 校验文件](https://github.com/Liuyy026/screen-capture-translator-public/releases/download/v0.21/SHA256SUMS.txt)

## 当前架构

```mermaid
flowchart TD
    UI["Swift · 文件夹、页面、编辑与任务界面"] -->|JSONL 管道| R["Rust · 常驻服务、队列、取消与恢复"]
    R --> O["Rust · 图片读取、OCR 页面装配、分镜与对白分组"]
    O --> P["Python · CTD / Manga OCR 模型推理"]
    P --> O
    O --> T["Rust · 普通/视觉提示词、图片编码、HTTP 与响应校验"]
    T --> L["外部模型服务 · oMLX / Ollama"]
    L --> T
    T --> C["Rust · 手改保护、原子保存与独立缓存"]
    C --> D["Rust · 气泡轮廓、文字蒙版、背景合成与显示缓存"]
    D --> M["Python · CTD / LaMa 模型推理"]
    M --> D
    D --> S["Swift · 中文横竖排版与最终绘制"]
```

| 层 | 当前职责 |
| --- | --- |
| Swift / SwiftUI | 阅读窗口、交互、连接设置、中文横竖排版与绘制 |
| Rust / 原生库 | 队列与取消、OCR 结果装配、结构分析、HTTP、图片编解码、缓存、清字与合成、自有引擎进程管理 |
| Python 模型适配器 | 已安装 CTD、Manga OCR、Anime LaMa 的推理及必要预处理/输出转换 |
| oMLX / Ollama | 语言或视觉模型服务；模型推理本身没有重写为 Rust |

Rust 按需启动和复用 Python 推理进程，并管理自己启动的本地 Ollama。已有 Ollama 和 oMLX 服务由用户管理。详细边界见 [核心说明](core/README.md)。

## 运行方式

要求 Apple Silicon、macOS 14+、Swift 编译工具、Rust/Cargo、原生 OpenCV 4.11.0，以及已准备好的 `macos/.venv` 和 `.models/`。**首次准备请按 [macOS 运行指南](macos/README.md) 完成依赖和模型安装**。

日常使用双击仓库根目录的 **[启动 Rust 候选版.command](启动%20Rust%20候选版.command)**，或 `macos/启动.command`。已运行时切回现有窗口，缺少应用或核心构建产物、仓库移动时自动构建。更新现有源码后，运行下面的开发命令重新构建。

开发时在本分支仓库根目录执行：

```sh
# 构建、启动候选版，并检查启动后持续存活 10 秒
./macos/scripts/build_and_run.sh --verify

# 已构建后直接打开
open 'macos/dist/Koma Rust 候选版.app'
```

`--verify` 只重启此 checkout 的候选应用；双击入口使用 `--open`，直接打开已有构建。构建产物依赖仓库路径及本机环境，移动仓库后需重新构建。

打开图片文件夹，在接口设置中保存服务地址、可选 Key 和模型，选择「普通翻译」或「视觉辅助」，再执行单页或整文件夹处理。视觉辅助结合漫画图像和已有 OCR 文字，翻译范围由已有文字框决定。

候选数据位于 `~/Library/Application Support/KomaReaderRust/`：`config.json` 保存连接配置，`pages/` 保存普通结果，`pages/vision-v1/` 保存视觉结果，`reading-native-v1/` 保存显示资产。首次可只读复制稳定版的配置和页面到独立副本。Key 可保存在权限 `0600` 的本地明文配置中，不访问钥匙串；不勾选保存 Key 时仅供本次运行使用。配置、原图、模型及私人评测工件不提交到 Git。

Windows 使用方法仍见 [Windows 说明](windows/README.md)，不能将其功能与性能视为已经迁入 Rust。

## 功能与显示效果

- 支持 WebP、PNG、JPG、JPEG、BMP、TIFF，原图只读，按文件名自然排序。
- macOS 支持单页和整章处理、单句重译、原文和译文编辑及手改保护。底部可切换显示和缩放、展开章节缩略图或收起对白栏。「本页诊断」汇总超分、识别、翻译与回填状态。
- 在右侧对白编辑区选择「保留原框」，可让整个已识别气泡保留原样，跳过翻译与回填。切回「翻译此框」可恢复处理。
- 「导出漫画」可将整章或指定页码合成为本地 PNG，保留章节原有页码。可指定保存位置与文件夹名称。操作方法见 [macOS 使用指南](macos/README.md#导出漫画)。
- 「处理设置」可启用 MetalFX 或 Real-CUGAN，支持按长边或高度放大到 2560 或 3840 像素。切换增强设置会复用同一翻译方式下的 OCR、译文和手工修改。Real-CUGAN 使用精确的全图特征同步，减少密集网点漫画偏色。参见 [超分说明](docs/macos-super-resolution.md)。
- 普通与视觉翻译分别缓存；画面复核建议需人工确认保存，不能自动改写日文。
- 中文横排按实际字宽和气泡轮廓换行，支持长句、相连或异形气泡及矩形旁白。窄长的日文竖排区域可自动改用中文竖排，也可在对白栏指定方向。
- 按文字像素蒙版清字，局部修补网点和复杂背景，连续旁白按顺序合并，不再铺整条白底。
- 显示与翻译缓存分开，已有译文可直接重新排版。无法回填时保留原图，在对白栏提供已有译文，并在底部状态区提示。

## 35 页对照结果（2026-11-08）

固定相同 35 页原图和既有译文，与重构前稳定版比较：

| 检查 | 结果 |
| --- | --- |
| 真实 OCR 与页面组装 | 259 个文字块及结构化结果一致 |
| 最终 Swift 整页渲染 | 35 页逐像素一致，布局审计一致 |
| 已有译文 | 244 段显示，新增漏排、字形越界、文字丢失均为 0 |
| 普通 / 视觉翻译流程 | 209 / 100 个差分场景通过，使用模拟模型回复 |
| 视觉输入图片 | 35 页及 56 个格式/尺寸/方向案例，JPEG 字节一致 |
| 控制器 / 真实窗口 | 8 / 6 项流程通过；另验证正式应用启动崩溃修复 |
| 原生基础与进程生命周期 | 11 项测试通过；重启服务后 35 页显示缓存全部命中 |

其余 15 个 OCR 块遵循既有跳过/保留规则，不算新增漏填。固定原图和原始译文缓存哈希未改变。第 13/15 页旁白、长句与异形气泡目视复核无相对基线回退。

| 显示处理指标 | Python 稳定版 | Rust 候选版 |
| --- | ---: | ---: |
| 35 页首次生成，含服务启动 | 98.035 秒 | 47.243 秒 |
| 缓存命中 105 次总计 | 13.259 秒 | 0.751 秒 |
| 缓存命中单页中位数 | 125.49 毫秒 | 6.54 毫秒 |

整章首次显示生成约快 2.1 倍。收益包含常驻进程和缓存复用，不能全部归因于 Rust。计时不含界面绘制，未强制清空系统文件缓存；**不是 OCR 或语言模型推理提速测试，也不是新一轮翻译质量评测**。内存统计口径不同，不能宣称内存降低；常驻模型会持续占用内存。

完整方法、测试入口及本地证据索引见 [验收报告](docs/rust-core-validation.md)、[迁移记录](docs/rust-core-migration.md) 和 [当时的进度](进度汇总/07_填充修复与Rust重构_2026-11-08/当前进度_2026-11-08.md)。这些数据对应迁移时的构建。

## 限制与后续

35 页一致性验收证明重构保留这组输入的效果，不能证明所有漫画均正确。OCR 漏字、手写字、姓名/日期错误与翻译语义问题仍需核对；背景修补也可能留有纹理痕迹。本轮保留原识别与译文，未混入内容校正。

Windows 继续使用原有阅读器。手动补框和拆框、跨页人物记忆、术语表、ZIP/PDF 输入与递归子目录仍待实现。

## 目录与许可证

```text
core/                   Rust 核心、原生桥与 Python 模型适配器
macos/                  Swift 界面、候选构建及模型环境
windows/                原 Python Windows 阅读器
backend/                原 Python 后端，保留作稳定版/差分参照
tools/rust_migration/   Rust 构建及离线核心回归
scripts/                模型准备、缓存迁移及回归入口
docs/                   架构、迁移和验收记录
进度汇总/               当前进度与分日期历史归档
```

使用 [Mokuro](https://github.com/kha-white/mokuro)、[Manga OCR](https://github.com/kha-white/manga-ocr)、[Comic Text Detector](https://github.com/dmMaze/comic-text-detector)、[IOPaint / Anime LaMa](https://github.com/Sanster/IOPaint)、[OpenCV](https://github.com/opencv/opencv)、[Ollama](https://github.com/ollama/ollama) 等上游组件，各自遵循其许可证。Rust 图片兼容实现的来源与 Pillow 许可见 [第三方说明](core/THIRD_PARTY.md)。模型和依赖不纳入 Git。

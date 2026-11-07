# macOS 安装与翻译流程验证

验证日期：2026-11-07。验证设备为 Apple Silicon，macOS 27.0.1；当前项目副本已构建、安装依赖及模型，并通过界面实际操作验证。

## 使用入口

双击 `macos/启动.command`，或打开 `macos/dist/Koma 漫画翻译.app`。本次交付时应用保持打开，选择了第一张样例并显示中文覆盖，本地模型就绪。开发构建入口为 `./macos/scripts/build_and_run.sh --verify`；Codex Run 按钮调用同一脚本。

## 修复与安装

- 用 `EditorState` 别名明确选择 SwiftUI 的状态属性包装器，避免 macOS 27 SDK 的同名宏依赖。未安装或修改系统 Xcode。
- 构建使用 `xcrun swiftc`，Apple Silicon 目标最低 macOS 14.0；实际可执行文件的 `LC_BUILD_VERSION` 已核对。只在当前 macOS 27.0.1 上实测，未实测旧系统。
- 应用已构建并通过本地签名校验（非公证发行包）。
- Python 3.12.14，项目专用环境位于 `macos/.venv`。43 个依赖通过 `uv pip check`。
- Manga OCR 0.1.16 要求 Transformers 至少 4.45，原 4.44.2 锁文件冲突；现固定为 Transformers 4.45.2、PyTorch 2.5.1、Torchvision 0.20.1。
- SciPy 1.15.3 的 PROPACK 扩展在本机被系统加载器拒绝；更新至 SciPy 1.17.1 后，`scipy.sparse.linalg` 导入及实际 OCR 均通过。NumPy 保持 1.26.4。
- OCR 模型位于根目录 `.models/`。本次真实识别日志包含 `Using MPS`。
- 项目专用 Ollama 0.34.0 位于 `macos/.runtime/ollama/`，下载包按官方 SHA-256 校验；模型 `qwen3:4b`（Q4_K_M）已安装，仓库位于 `.models/ollama/`。
- 私有服务使用 `127.0.0.1:11439`，云端功能关闭。真实翻译时 `/api/ps` 返回 GPU 内存占用约 3.18 GB、上下文 4096，证明调用了已加载的本地模型。
- 用户已手动允许 macOS 的“桌面”访问权限；本次未代替用户修改系统隐私设置。

上游依据：[Manga OCR 0.1.16 发布记录](https://github.com/kha-white/manga-ocr/releases/tag/v0.1.16)、[SciPy PROPACK 加载问题](https://github.com/scipy/scipy/issues/25635)。版本适配以本机导入及真实流程结果为验收依据。

## 实际流程

1. 从应用窗口通过文件夹选择器打开 `samples/`，WebP 图片正常显示。
2. 第一张样例从无缓存开始，通过界面“只识别本页日文”识别 5 个文字块（4 句对白和 1 个页眉），横排与竖排对白均识别正确。
3. 通过界面“识别并翻译”实际调用 Qwen3 4B，5 个块均返回结果，错误字段为空；右侧日中对照和中文覆盖正常显示。
4. 通过“翻译整个文件夹”处理两页，界面显示完成 2 页；共 6 个块均有结果且错误字段为空。切页后能读取相应缓存。
5. 将第二张样例无损另存为带独立元数据的临时 PNG，确认其内容哈希对应的缓存不存在，再以 `--operation both --model qwen3:4b` 调用工作进程。真实 OCR → 翻译 → 缓存写入完成，耗时约 9.35 秒，结果为“准备好了，走吧！”。该数值仅代表这张简单样例，不是通用性能承诺。
6. 退出应用后确认它启动的 Ollama 服务停止；随后通过普通 `macos/启动.command` 重新启动，应用自动重启专用服务，恢复缓存并显示“本地模型就绪”。

两张原始样例均核对与 Git 中原始字节一致，没有修改或覆盖原图。所有样例缓存均为本次验证新建。

## 本次译文

| 图片 | OCR 原文 | 实际译文 |
| --- | --- | --- |
| samples/01.webp | 今日はいい天気だね | 今天天气真不错呢 |
| samples/01.webp | ＫＯＭＡ，ＯＯＲＴＥＳＴ．０１ | KOMA，OORTEST.01 |
| samples/01.webp | そうだね。 | 对啊 |
| samples/01.webp | 一緒に散歩しよう | 我们一起去散步吧 |
| samples/01.webp | ちょっと待って！ | 等等！ |
| samples/02.webp | 準備できたよ。行こう！ | 准备好了，走吧！ |

第一张的英文页眉 `KOMA / OCR TEST · 01` 被日文 OCR 识别为 `ＫＯＭＡ，ＯＯＲＴＥＳＴ．０１`，因此该块输出只是沿用了错误的 OCR 文本；不能据此声称英文识别准确。四句日文对白及第二张的日文识别、语义翻译均经对照样例核查。

## 回归与证据

- 共用后端运行 64 项测试：62 项通过，2 项因仓库未提供历史漫画素材而跳过。
- 依赖兼容检查、构建、签名、脚本语法和 Git 空白检查通过。
- 记录保存在 `macos/.runtime/acceptance/`：`verification-summary.json`、`sample-results.json`、`fresh-pipeline.json`、`translation-requests.jsonl`、`ollama-loaded.json` 及界面截图。
- 最终界面截图：`macos/.runtime/acceptance/final-ready.png`；第二页截图：`macos/.runtime/acceptance/02-chinese.png`。
- 安装日志：`macos/.runtime/install.log`；后端回归日志：`macos/.runtime/backend-tests.log`。界面每次处理的 OCR 日志仍使用 `macos/.runtime/ocr.log`，会被后一次处理更新，因此冷流程日志另存 `fresh-worker.log`。

上述本地运行证据、模型、虚拟环境和应用产物均被 Git 忽略。源码、依赖锁文件与说明保留为可审阅的本地修改，未提交或推送。Windows 启动器/CUDA 未在本次任务中实测；真实漫画的识别与翻译质量仍需按图片检查，中文覆盖继续沿用原有实现。

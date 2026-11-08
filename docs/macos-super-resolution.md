# macOS 前置可选超分

2026-11-08，Rust 候选版。两套本地后端已接入同一流程：

`原图 → 可选 MetalFX / Real-CUGAN → OCR → 翻译 → 清字修补 → Swift 中文绘制`

## 使用

退出已运行的 Rust 候选版后，用仓库根目录的 `启动 Rust 候选版.command` 重新打开。右侧「前置增强」可选关闭、MetalFX 空间超分、Real-CUGAN。目标为 1440 或 2160 像素，可按长边或高度计算；等比例放大，已达目标默认跳过。

- MetalFX 只提供标准预设，使用真实 spatial scaler，无需权重下载。
- Real-CUGAN 提供保守、标准、降噪，使用 models-se 对应模型和 2/3/4 倍推理，必要时 Lanczos 缩小到指定尺寸。超出最大 4 倍的请求明确失败。
- 初始关闭。增强选项为当前会话设置，重开应用后需重新选择。
- 开启后「增强原文」显示增强后的原文页面，「中文」显示基于该图识别与翻译的结果。
- 切换后端、尺寸或预设时使用各自的识别和译文缓存。首次选择新的处理方式，需要重新执行识别/翻译；原图的人工修改不会自动迁移到新结果，也不会被覆盖。
- 停止按钮覆盖页面增强及处理队列。失败时保留原图并说明错误，不静默切换后端。

此版本不提供后置增强，不把两次超分串联使用。原图文件始终只读。

## 文件与运行依赖

- `macos/tools/MetalFXUpscaler.swift` → `macos/.build/koma-sr-metalfx`。
- `macos/tools/RealCUGANUpscaler.swift` → `macos/.build/koma-sr-realcugan`，调用原生 arm64 ncnn/MoltenVK 程序。
- `core/src/super_resolution.rs` 管理统一协议、尺寸、缓存、输出校验、超时与取消；Rust 在 OCR 前选择有效图。
- `macos/scripts/build.sh` 构建两套 helper 和候选应用。Real-CUGAN 资源缺失不影响编译，但能力检查会显示不可用。
- 初次准备 Real-CUGAN：`bash macos/scripts/prepare_realcugan.sh`。发行包 SHA256 固定，资源仅写入本 checkout 的 `.runtime/super-resolution/realcugan/`。本机已从独立验证副本复制已校验的引擎、模型和来源清单，无需再次下载。
- 超分运行不依赖 Python；现有 OCR、背景修补仍使用其原有 Python 模型适配器。准备脚本和测试脚本可以使用 Python。
- 仍是依赖项目目录的本地候选版，并非可独立分发的安装包；没有修改稳定版或 Windows。

默认候选数据目录为 `~/Library/Application Support/KomaReaderRust/`。增强图片在 `super-resolution-v1/<identity>/effective.png`，增强 OCR/译文在 `pages/enhanced-v1/<identity>/`，视觉模式另有 `vision-v1/` 子目录。缓存身份包含输入内容、设置、实际尺寸、helper 和模型/系统身份；命中时检查输出内容哈希及尺寸。程序中断留下的临时文件不会作为有效结果使用。

## 整合验收

实际环境：Apple M5 Max，macOS 27.0.1 (26A434)。两个独立交付提交为 `69affc4`（MetalFX 与公共接入）和 `addcbe3`（Real-CUGAN）；主候选版分别整合为 `69134d0` 和 `4ee3531`，随后补充双后端构建与联合验证。

主验收数据在本 checkout 的 `.runtime/super-resolution/`，不提交漫画、模型或输出图片：

| 检查 | 结果 | 证据 |
| --- | --- | --- |
| 两个 helper 相同输入/目标 | 8 个输出精确尺寸，源文件不变 | `integrated-helpers/report.json` |
| 原图及符号链接覆盖保护 | 4 个拒绝覆盖检查通过 | 同上 |
| Rust 单元测试 | 12 通过 | `integrated-unit-tests.log` |
| 关闭/MetalFX/Real-CUGAN 的真实前置链路 | 三条流程均完成真实检测、OCR、翻译接口和清字显示 | `integrated-flow/report.json` |
| 图像与坐标一致 | 原图 389×540；两种增强均 1037×1440，OCR 记录的尺寸一致 | 同上 |
| 缓存与手改保护 | 三份独立缓存；原图缓存人工修改未变；增强普通 OCR 可复用于对应视觉缓存 | 同上 |
| 有效译文完整性 | 三种结果都覆盖本样本 19 个识别块，无遗漏 | `integrated-display-audit.json` |
| 真实 SwiftUI 窗口切换 | MetalFX、Real-CUGAN、关闭三状态显示与尺寸检查通过 | `integrated-window-final/report.json`，截图同目录 |
| 原始输入保护 | 35 张原图与其基线/稳定翻译缓存，共 105 个文件内容不变 | `source-protection-check.json` |

整合测试的翻译服务是本地固定回复，不连接共享 oMLX/Ollama，不代表语言模型翻译质量通过。真实 OCR 与背景模型参与，固定译文仅用于验证端到端传递、清字与排版。

独立交付还验证了原有 35 页流程、请求取消与进程树回收、快速切页、缓存损坏修复、方向处理、资源缺失等，详情见 [MetalFX 交接](super-resolution-metalfx-handoff.md) 与 [Real-CUGAN 交接](super-resolution-realcugan-handoff.md)。

## 本轮效果与耗时

联合测试使用第 20 页人工缩小到高 540/720/1080，再输出高 1440/2160。四个样例/后端的进程墙钟（包含初始化、解码、模型执行与 PNG 写出）：

| 输入高度→目标高度 | MetalFX | Real-CUGAN 保守 |
| --- | ---: | ---: |
| 540→1440 | 0.101 s | 2.281 s |
| 540→2160 | 0.139 s | 0.746 s |
| 720→2160 | 0.146 s | 0.814 s |
| 1080→2160 | 0.144 s | 0.902 s |

仅为本轮运行，未清空系统缓存，不是纯 GPU 时间、稳定速度承诺或完整章节基准。首次与后续运行成本不同。全流程约 13–15 秒的数值包含 OCR/背景模型和初始化，不能与上表超分阶段混用，也不能用于宣称加速。

已查看联合文字裁剪 `integrated-helpers/text-comparison.png` 和三个状态的真实界面截图：MetalFX 边缘更锐，但低清文字周围可能出现明显锐化痕迹；Real-CUGAN 字形轮廓更干净，但细碎网点/排线会被平滑或重组。540 输入中已经丢失的笔画没有得到可靠恢复。1080 输入的文字较清楚，增强仍会改变部分纹理。

两种增强在联合样本中都识别到 19 块，与低清原图数量一致；数量相同不等于文字正确率相同。本轮没有人工字符级标注，也没有证明 OCR 准确率提升。人工降采样只用于配对观察，不能代替真实低清下载素材的验收。

## 复现联合验证

在仓库根目录执行，输出目录应选新的独立目录：

```sh
zsh macos/scripts/build.sh
python3 tools/rust_migration/cargo.py test --quiet
PYTHONDONTWRITEBYTECODE=1 macos/.venv/bin/python tools/super_resolution/verify_integrated_helpers.py \
  --root "$PWD" --source /absolute/source.webp --output /absolute/new-helper-review --lock /absolute/gpu-test.lock
PYTHONDONTWRITEBYTECODE=1 macos/.venv/bin/python tools/super_resolution/verify_front_pipeline.py \
  --root "$PWD" --input '/absolute/new-helper-review/输入 页面 高540.png' \
  --output /absolute/new-flow-review --lock /absolute/gpu-test.lock
PYTHONDONTWRITEBYTECODE=1 python3 tools/rust_migration/run_sr_review.py \
  --flow /absolute/new-flow-review --runtime-subdir runtime \
  --image '/absolute/new-helper-review/输入 页面 高540.png' --output /absolute/new-window-review \
  --engines metalfx,realcugan,off --axis height --lock /absolute/gpu-test.lock
```

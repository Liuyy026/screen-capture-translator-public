# Koma · macOS

简体中文 | [English](README.en.md) | [项目首页](../README.md)

本页说明 `refactor/rust-core` 的独立候选应用。架构和验收摘要见[项目首页](../README.md)，稳定版仍位于 `main`。以下命令均在仓库根目录执行。

## 首次准备

要求 Apple Silicon、macOS 14+、可用的 `xcrun swiftc`、Rust/Cargo、Python 3 和 uv。候选应用还依赖项目内原生 OpenCV、Python 3.12 环境和模型权重，不是独立安装包。

先构建项目内 OpenCV 4.11.0，不写入系统库目录（首次会下载源码和构建工具）：

```sh
python3 -m venv .runtime/build-tools
.runtime/build-tools/bin/python -m pip install cmake==3.31.6 ninja==1.11.1.4
python3 tools/rust_migration/build_opencv.py \
  --directory .runtime/native-deps \
  --cmake .runtime/build-tools/bin/cmake \
  --ninja .runtime/build-tools/bin/ninja
```

若已经有本项目可用的 `macos/.venv` 和 `.models/`，无需重复安装。新环境可选择下列一种准备方式：

**使用已有 oMLX 服务：**只准备识别与修补依赖，oMLX 及其视觉/语言模型由用户在现有服务中配置。

```sh
uv venv --python 3.12 macos/.venv
uv pip install --python macos/.venv/bin/python -r macos/requirements.lock
macos/.venv/bin/python scripts/prepare_models.py
python3 macos/scripts/setup_reading_model.py
```

以上命令会下载 OCR/检测及约 197 MiB 的背景修补权重。已安装的权重由运行时离线读取，不会在阅读时自动下载。

**需要项目内 Ollama 和 Qwen3 4B：**在 OpenCV 准备完成后执行旧安装器，再安装背景模型：

```sh
python3 macos/scripts/setup.py
python3 macos/scripts/setup_reading_model.py
```

此安装器会建立 Python 环境、下载 OCR、Ollama 与 Qwen3 4B，必要时启动本地 Ollama 完成模型下载，最后构建候选应用。仅使用现有 oMLX 时不需要此步骤；安装器也不会代为安装 Rust 或构建原生 OpenCV。安装完成后可使用下方的候选版双击入口。

## 构建与启动

日常双击仓库根目录的 **[启动 Rust 候选版.command](../启动%20Rust%20候选版.command)**，或本目录的 **启动.command**。两者共用 `scripts/build_and_run.sh --open`：已构建就直接打开，已运行就切回窗口；缺少应用/核心构建产物或移动仓库后自动构建并检查启动。首次依赖未准备时会保留具体错误，提示按本页完成准备，不自动安装或下载模型。

```sh
# 构建、启动，并检查进程持续存活 10 秒
./macos/scripts/build_and_run.sh --verify

# 只构建，不启动应用或翻译服务
zsh macos/scripts/build.sh

# 打开已构建的候选版
open 'macos/dist/Koma Rust 候选版.app'
```

输出应用为 `Koma Rust 候选版.app`，标识 `local.koma.reader.rust`。两个双击入口均指向该候选应用。开发用 `--verify` 会重新构建并重启本 checkout 的候选版，不覆盖稳定应用；`--logs` 查看日志，`--debug` 进入 LLDB。应用通过构建时记录的仓库绝对路径使用核心、环境和模型，移动仓库后须重新构建。

首次访问桌面中的漫画或运行环境时，请完成 macOS 文件夹授权。`--verify` 是启动检查，不代替实际翻译或界面验收。

1. 点击「打开文件夹」，选择直接包含图片的文件夹。
2. 点击「识别并翻译」。也可以通过「更多」菜单只识别日文，或翻译整个文件夹。
3. 切换「原图 / 文字框 / 中文」查看不同显示模式。
4. 在右侧编辑文本，点击「保存修改」；修改日文会清除对应旧译文。
5. 重新翻译单句前先保存编辑，再点击该句下方的「重新翻译」。

仓库根目录 `samples/` 中的两张原创 WebP 测试图包含横排和竖排日文，可用于检查流程，不代表真实漫画上的识别准确率。


## 开发、数据与验证

Swift 的 `NativeCoreService.swift` 通过 JSONL 管道连接 `core/target/release/koma-core serve`。Rust 负责队列、缓存、翻译请求、页面分析及背景合成；`core/python/model_worker.py` 仅运行 CTD/Manga OCR/LaMa 推理；Swift 保留排版与绘制。旧 `backend/` 和 `macos/backend/` 是稳定实现及差分参照，不是候选阅读器的业务执行入口。

| 路径 | 用途 |
| --- | --- |
| `macos/.venv/`、`.models/` | Python 模型环境、已安装权重 |
| `.runtime/native-deps/`、`core/target/` | 原生 OpenCV、Rust 构建产物 |
| `macos/.runtime/ollama/` | 可选的项目内 Ollama |
| `~/Library/Application Support/KomaReaderRust/config.json` | 候选版连接配置 |
| 同目录 `pages/`、`pages/vision-v1/` | 普通/视觉结果与手工编辑 |
| 同目录 `reading-native-v1/`、`native-core.log` | 原生显示缓存、服务日志 |

首次可只读复制 `KomaReader` 中的稳定配置和对应页面，之后写入候选独立目录。稳定数据不会随候选编辑变化，普通和视觉模式的编辑也不自动互相同步。

已准备原生依赖后，可运行不需要真实模型权重或翻译服务的基础检查：

```sh
python3 tools/rust_migration/cargo.py test
macos/.venv/bin/python -m unittest discover -s core/python -p 'test_*.py'
```

35 页验收已完成真实 OCR 对照、完整显示像素对照、模拟 HTTP 流程验证、控制器及真实窗口检查。详细数字与计时口径见[验收报告](../docs/rust-core-validation.md)；复测入口见[核心说明](../core/README.md)和[最新进度](../进度汇总/当前进度_2026-11-08.md)。私人漫画、缓存及 `.runtime/rust-migration/pages.json` 未提交，克隆仓库不附带这套 35 页测试数据。

## 翻译服务：Ollama / oMLX

在阅读器中打开接口/服务设置，选择服务，填写地址与可选 API Key，获取模型列表、选择模型后保存。oMLX 默认为 `http://127.0.0.1:8000/v1`，Ollama 默认为 `http://127.0.0.1:11439`。oMLX 需自行运行，应用不会启停它。

开发期间，服务、接口地址、模型和可选 API Key 保存在 `~/Library/Application Support/KomaReaderRust/config.json`，不放入仓库。配置是明文文件，权限为 `0600`，仅当前用户可读写；每次保存均原子替换。设置中的「将 API Key 保存在本地配置文件」不勾选时，密钥只用于本次运行，并从该服务的持久配置中清除。应用不会读取、写入或弹出系统钥匙串授权；钥匙串接入留待稳定后再处理。

首次没有候选配置时，可从稳定版本地配置只读导入独立副本；没有稳定配置则使用已有偏好中的服务、地址和模型，不读取旧钥匙串密钥。JSON 格式错误、版本不支持或权限异常会显示具体文件位置与修复提示，并阻止自动连接和覆盖原配置。手工修改前先退出应用；修复文件，或将它移走保留备份后，重新启动。

配置格式示例（不需要认证时 `apiKey` 填空字符串；`connections` 每个服务最多一项）：

```json
{
  "version": 1,
  "selectedProvider": "omlx",
  "connections": [
    {
      "provider": "omlx",
      "baseURL": "http://127.0.0.1:8000/v1",
      "model": "your-local-model",
      "apiKey": ""
    }
  ]
}
```

切换服务后重新生成机器译文，手改译文继续保留。操作、CLI 参数和真实样例记录见[接入与验证说明](../docs/omlx-integration-2026-11-07.md)；该历史记录中的钥匙串说明已由上述本地配置方案替代。

无需启动应用或模型即可运行本地配置检查：

```sh
mkdir -p macos/.build
xcrun swiftc -parse-as-library macos/Sources/TranslationLocalConfig.swift macos/tests/ConnectionConfigTests.swift -o macos/.build/connection-config-tests
macos/.build/connection-config-tests
```

## 鼠标拖动图片

在图片上按住鼠标左键并拖动，即可平移查看超出视窗的部分；从原图或译文区域开始拖动都可以。放大、缩小或换页后仍可继续拖动。短按文字框仍用于定位对白，滚轮和触控板也可继续滚动。

拖动会限制在图片边界内；缩小到整张图片都能显示时，图片保持原位。

## 视觉辅助翻译（oMLX）

右侧「翻译方式」可选择「普通翻译」或「视觉辅助」。后者保留现有文字检测和 Manga OCR，将整页图片、文字坐标与日文一起交给 oMLX 视觉模型；需要服务中已有支持图片的模型。当前优先使用本机 `Qwen3.8-27B-Uncensored-MLX-4bit`，仍可通过模型菜单更换。

选择「视觉辅助」后，点击「识别并翻译」，也可只识别、重译单句或批量处理。原文与中文仍可手动编辑；图片中发现的疑似 OCR 错误显示为「画面复核建议」，点击填入日文后，核对并保存才会修改原文。中日共用词的相同译文会保留并提示核对。

视觉结果保存到 `~/Library/Application Support/KomaReaderRust/pages/vision-v1/`，普通结果仍在 `pages/`。两种模式单独保存编辑；首次建立视觉结果时，可复用普通模式已有 OCR 和手动译文，后续修改不自动跨模式同步。图片长边最多 1775 像素；当前仍依赖 OCR 找框，漏检文字、拟声词不会自动补框。视觉模式不保证比普通翻译更准确，也不保证模型不会拒译。

候选版实现位于 `core/src/vision_pipeline.rs`、`vision_image.rs` 和 `vision_decode.rs`，由 Rust 完成请求、编码及保存；Windows 未迁移。[早期真实模型试用记录](../docs/macos-vision-1209806-2026-11-07.md)保留当时的 Python/钥匙串/白底方案，不代表当前实现。Rust 迁移使用确定的模拟回复做流程对照，没有重新进行真实语言模型质量评测。

## 中文气泡排版

「中文」模式在打开已有结果时，单独分析气泡和原文字范围，不调用翻译服务，也不改写漫画或译文缓存。同一气泡的碎片文字合并排版；有狭窄连接的双气泡会尝试分开。支持矩形旁白框、页底被截断的气泡；无封闭轮廓时，只在原文字范围内尝试排版。原图/文字框模式仍按原 OCR 框显示。

中文默认从左到右、从上到下横排。每行按气泡的实际轮廓寻找可用宽度，长句可以利用异形气泡的安全空间，不局限于单个内接矩形；不同译文保留段落分隔。字号以页面宽度统一计算，并保留可读下限。点击中文区域可定位其第一段对白，右侧仍可逐段编辑。

清字复用已安装的 Comic Text Detector 的逐像素文字蒙版，并限制在已翻译文字范围和气泡轮廓内。白底气泡清除笔画、标点和长划线；连续旁白按阅读顺序合并为横排段落，检测分镜线以避免跨栏合并。中文使用实际字宽换行、细描边增强对比，不铺长条纯白底。

复杂背景可使用本地 Anime LaMa 修补模型（约 197 MiB），只重建文字蒙版内的像素，保留蒙版外的画面。首次需要显式安装；阅读时不会联网下载，也不会启动 oMLX/Ollama：

```sh
python3 macos/scripts/setup_reading_model.py
```

权重保存于 `.models/anime-manga-big-lama.pt`，下载后核对发布方提供的校验值。来源：[IOPaint AnimeLaMa](https://github.com/Sanster/IOPaint/blob/main/iopaint/model/lama.py)。没有该模型时使用局部传统修补，复杂网点效果较弱。修补是在估计原字遮住的背景，仍可能留下模糊或纹理痕迹，不等于精确恢复原画。

显示结果和文字蒙版单独保存在 `~/Library/Application Support/KomaReaderRust/reading-native-v1/`；图片、译文、显示算法或模型变化后自动重建显示结果，已经翻译的旧页面无需重译。纯省略号、相同的数字保留原样，不算漏填；疑似页码误识别会保留原图并提示，手动核对保存后可覆盖。

旧缓存中的白色矩形不会直接作为回退覆盖；打开页面时重新检查背景和文字范围。译文不完整、排版空间不足或背景不可靠时保留原图，右侧明确提示“已有译文暂未覆盖”，并显示完整译文。此项改善显示，不自动修正 OCR 或翻译语义，也不保证手写字、复杂背景、漏检文字和所有异形气泡都能覆盖。

候选版显示分析入口为 `core/src/reading.rs`，几何、清字、合成分别位于 `geometry.rs`、`background.rs` 和 `texture.rs`；Swift 使用 `ReadingLayout.swift` / `ReadingOverlayView.swift` 排版绘制。`macos/backend/reading_overlay.py` 和 `macos/tools/review_chapter.py` 保留作 Python 参照工具，其默认缓存路径仍属于稳定版，不能用它们证明 Rust 候选流程通过。

Rust 对照工具位于 `tools/rust_migration/`：`verify_display.py` 检查完整显示输出，`verify_rendered_pages.py` 检查最终 Swift 像素，`run_reader_flow.py` 检查独立窗口流程。它们需要本地准备的 manifest、基线和已安装模型；运行会生成独立测试结果，不提供产品界面的汉化导出功能。

模型复用会保留内存；退出候选应用时清理所属推理进程。Rust 只管理自己启动的、无密钥且位于既定 `127.0.0.1:11439` 地址的本地 Ollama；已有 Ollama/oMLX 服务不被接管。

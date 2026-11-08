# 前置可选超分：MetalFX 与公共集成交接

日期：2026-11-08。独占副本：`/Users/lyy-studio/Desktop/project111/本地ai翻译/超分候选实验/20261108-front-sr/metalfx`。
基线：`d24a9325699d8b9102c354b46025ed42dce22996`。交付提交为包含本文件的本副本 Git HEAD；交接时可 `git rev-parse HEAD` 获取。

## 已完成

- `macos/tools/MetalFXUpscaler.swift`：真实 Apple MetalFX **spatial**，检查设备能力；没有 temporal 模拟或 CPU 插值替代。sRGB BGRA8 perceptual 输入/输出，Core Image 解码并应用图片方向。等待 GPU command 完成，检查错误与输出尺寸，生成新 PNG；拒绝覆盖输入、已有输出及不支持的预设。
- MetalFX 仅支持 `standard`。`conservative` / `denoise` 明确非零退出，不假称具备降噪能力。能力结果包含 engine/available/presets/reason/backend_version、系统 build 和 GPU 名称。MetalFX 使用系统框架，无下载权重。
- `core/src/super_resolution.rs`：统一 helper 协议、1440/2160 长边或高度等比尺寸、达标跳过、可取消 GPU 门控、临时目录与原子发布、180 秒超时、独立进程组及取消时杀掉本任务进程树。`load_page` 与 `process` 都使用它。
- 缓存身份包括：原图 SHA256、设置、实际尺寸、完整 capabilities（系统/后端/模型身份）、helper 可执行文件 SHA256。identity sidecar 保存实际输出 SHA256；命中时检查 identity、内容 hash 与解码尺寸；相同尺寸篡改/sidecar 损坏会重新生成。增强期间原文件变化拒绝发布。
- `runtime.rs`：增强页缓存放 `pages/enhanced-v1/<identity>/[vision-v1/]`，增强图放同一候选数据目录下 `super-resolution-v1/<identity>/effective.png`。不从稳定/原图缓存迁移手改内容。增强普通 OCR 到视觉模式仅从对应增强普通缓存准备。
- `server.rs`：新增 `sr_capabilities`；`load_page` 与 `process` 可带 `super_resolution`；加载响应额外含 `original_image`、`effective_image`。OCR、视觉/普通翻译和清字均使用有效图；事件中的文件身份仍是原图。
- Swift：前置增强引擎、目标、长边/高度和预设选择；文件列表和 selected 保持原始路径，显示/清字使用 effective image。设置切换取消旧任务，并用请求 generation 拒收旧进度、页面、图像和清字结果。停止按钮覆盖打开页时的增强与处理任务。取消后始终释放 busy/task 所有权，旧请求不会自动重新启动增强。
- `models.rs`：模型 Python 禁止生成共享依赖目录的 bytecode。
- 构建脚本新增可选 `KOMA_APP_ID` 与 `KOMA_CACHE_NAMESPACE`，默认行为保留。验证 bundle 为 `local.koma.reader.rust.metalfx-test`，缓存域 `KomaReaderRustMetalFXTest`。未安装、覆盖或启动稳定应用；未调用共享 oMLX/Ollama 服务。

## Real-CUGAN 合入接口

预留 `macos/.build/koma-sr-realcugan`，MetalFX 路径为 `macos/.build/koma-sr-metalfx`。Rust 当前按模型 worker 所在项目根目录定位两个 helper；worker 应为本项目 `core/python/model_worker.py`。没有 Real-CUGAN 时能力准确显示“尚未准备”，请求增强明确失败；不会回退到 MetalFX 或原图伪造成功。若原图已达尺寸，按设置跳过，不要求运行引擎。

协议：

```text
helper --capabilities
helper --input ABS --output ABS --width W --height H --preset conservative|standard|denoise
```

stdout 为单个 JSON；日志到 stderr；失败退出非零。成功 JSON 至少 engine/width/height/elapsed_ms。能力至少 engine/available/presets/reason，并需稳定 backend_version 与系统或模型 hash 字段。全部能力会构成缓存身份。调用方提供新的临时 PNG 路径。Real-CUGAN helper 自己负责公开准确的可用预设和模型身份。合入其源码/安装脚本后构建到预留路径即可；不需要修改此公共调度。

示例 JSONL 请求字段：

```json
{"id":"example","op":"load_page","image":"/absolute/original.png","mode":"text","super_resolution":{"engine":"metalfx","target":1440,"axis":"long_edge","preset":"standard"}}
```

关闭为 `engine: off` 或省略字段。关闭直接沿用原始输入和基线页缓存布局。界面设置当前为会话设置，未新增持久化格式。

## 实机与效果边界

- 实际硬件：`Mac17,14`，Apple M5 Max，64 GiB；macOS 27.0.1 (26A434)。MetalFX 能力检查 available=true。
- 后端：`koma-metalfx-spatial-v1`。helper SHA256：`1d7a61271c8a6a23a982d59936fcf62f2eccc4231af467d8265955a5fe84ba37`。
- `.runtime/super-resolution/model-identities.json` 保存 helper、comictextdetector、manga-ocr、LaMa 实际模型路径与 SHA256。
- 只读素材来自主项目 `.runtime/rust-migration/pages.json` 前两页。540/720/1080 是明确的**人工降采样**，分别增强到长边 1440/2160，共 12 个配对输出；不能据此推出对真实低清扫描件的收益。
- 最终批次 helper elapsed：84.3–170.6 ms，中位 116.4 ms；进程 wall 92.4–180.7 ms。helper elapsed **包含本次进程的解码、初始化、GPU、PNG 写出**，wall 还包含进程启动。不是纯 GPU 时间或稳定帧率。所有实际推理计时都持有两个对话共用的 `gpu-test.lock`。
- 已直接查看 reference/MetalFX/bilinear 相同裁剪。MetalFX 相比 bilinear 边缘更锐，但细网点和线条不能恢复原始信息；有的配对 MAE 更低，有的更高。报告保留全部数据，没有用分辨率或 MAE 单独宣称视觉质量提升。
- 真实完整流程只在一张人工 720 长边漫画页对比：原图 519×720 和 MetalFX 1038×1440 都识别 8 个块；真实 detector/manga-ocr/LaMa 参与，OCR 后经**本地固定回复翻译接口**写入“测试中文”，再生成清字/显示并用真实 Reader 界面截图。此测试证明图像/坐标/缓存/接口链路，不是翻译质量评价。耗时约 7.79 s 与 4.95 s，包含惰性初始化且顺序不同，不能作为加速结论。
- 原始漫画与稳定页缓存 hashes 均核对不变；没有上传素材。没有实际自然低清集、人工 OCR 正确率标注、成人内容保证或全章质量验收。

## 已运行验证与产物

以下路径均相对于本副本；`.runtime`、模型、漫画和构建产物均不提交。

| 检查 | 结果与证据 |
| --- | --- |
| Rust 单元测试 | 12 passed，0 failed：`.runtime/super-resolution/logs/koma-metalfx-final-tests3.log` |
| 候选 app 构建、签名 | 通过：`dist/Koma Rust 候选版.app`，构建日志在 `logs/koma-metalfx-build-final.log` |
| 原有 Rust runtime 回归 | 35 页缓存/队列/编辑/HTTP 重试/活动与排队取消/子进程回收/异常恢复/EOF，通过：`runtime-regression/report.json` 与日志 |
| 原有 Reader 状态回归 | 8 场景通过，包含 HTTP 取消后可重试、35 页队列与快速切页：`reader-baseline-final/report.json` |
| MetalFX/helper/新缓存回归 | 方向/RGB、拒绝覆盖/不支持预设、12 输出、损坏修复、并发命中、普通增强 OCR→视觉模式、原图手改保留、达标跳过：`verification-final/report.json` |
| 进程树取消 | `load_page` 和处理队列中的增强均取消，helper 与 descendant 消失、无晚到页结果、临时目录清理：`cancellation-both-final/report.json` |
| Swift 增强状态 | 打开页取消、快速切页/目标切换、源列表保留、关闭恢复、缺失后端与恢复，通过：`reader-tests/reader-report.json` |
| 真 OCR→翻译接口→清字 | `real-flow-02/report.json`，两版 OCR/page/display JSON 与有效图可直接检查 |
| 真实窗口 | `window-review-final/report.json`；`metalfx-chinese.png`、`original-chinese.png` 均已查看；后端能力/隔离和中文展示可见 |

主产物根目录：`.runtime/super-resolution/`。`verification-final/` 包含完整输出和三个版本的同坐标 crop，报告保存输入输出绝对路径、尺寸、耗时与 MAE。`window-review-01` 是早期窗口抓图透明背景问题，最终可用截图请用 `window-review-final`。`real-flow-01` 为早期测试报告字段名错误导致中止，真实流程结论以已通过的 `real-flow-02` 为准。`cancellation-01` 为测试 fixture 路径错误导致未完成的早期运行，最终结论用 `cancellation-both-final`。

## 复现命令

在副本根目录执行；输出目录必须尚不存在。Python 的模型/图像测试用只读链接的现有 venv；不安装依赖。

```sh
KOMA_APP_ID=local.koma.reader.rust.metalfx-test KOMA_CACHE_NAMESPACE=KomaReaderRustMetalFXTest macos/scripts/build.sh
python3 tools/rust_migration/cargo.py test --quiet
PYTHONDONTWRITEBYTECODE=1 macos/.venv/bin/python tools/rust_migration/verify_super_resolution.py --manifest .runtime/rust-migration/pages.json --output .runtime/super-resolution/recheck
PYTHONDONTWRITEBYTECODE=1 macos/.venv/bin/python tools/rust_migration/verify_sr_cancellation.py --image .runtime/super-resolution/verification-01/page02-artificial-720.png --output .runtime/super-resolution/cancel-recheck
PYTHONDONTWRITEBYTECODE=1 macos/.venv/bin/python tools/rust_migration/verify_sr_flow.py --image .runtime/super-resolution/verification-01/page02-artificial-720.png --output .runtime/super-resolution/flow-recheck
PYTHONDONTWRITEBYTECODE=1 python3 tools/rust_migration/run_sr_review.py --flow .runtime/super-resolution/real-flow-02 --image .runtime/super-resolution/verification-01/page02-artificial-720.png --output .runtime/super-resolution/window-recheck
```

原有 Reader 完整回归命令：

```sh
PYTHONDONTWRITEBYTECODE=1 python3 tools/rust_migration/verify_reader_state.py --manifest .runtime/rust-migration/pages.json --assets /Users/lyy-studio/Desktop/project111/本地ai翻译/screen-capture-translator-rust/.runtime/rust-migration/reader-state-01/reading-native-v1 --output .runtime/super-resolution/reader-state-recheck
```

manifest 为本副本 `.runtime/rust-migration/pages.json` 的只读内容副本；assets 参数来自主项目，测试自行复制到独立输出，不修改主项目。Swift 新测试编译方式同现有 harness：`-D KOMA_REVIEW_HARNESS macos/Sources/*.swift macos/tests/SuperResolutionReaderTests.swift`，链接 SwiftUI/AppKit/CryptoKit/Security，参数为副本根、独立输出目录、两张人工720图片。

Apple API依据：<https://developer.apple.com/documentation/metalfx/mtlfxspatialscalerdescriptor>、<https://developer.apple.com/documentation/metalfx/mtlfxspatialscaler>。系统框架能力和预设限制以实际设备检查为准。

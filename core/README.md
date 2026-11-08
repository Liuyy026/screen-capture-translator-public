# Koma Rust core（重构进行中）

本目录是独立分支的实验核心，完整显示生成已通过 35 页像素对照，候选应用已完成主要调用接入并编译，真实界面验收尚未完成，暂不能替代稳定阅读器。目标职责与验收标准见 [迁移记录](../docs/rust-core-migration.md)。Python 模型进程仅运行检测、OCR 和背景修补模型；不能依赖旧 Python 业务流程完成所谓 Rust 处理。

## 本地开发

使用 Rust 工具链以及现有、已安装模型的 Python 环境。下列基础测试无需重新下载模型，不读取 API Key，不访问翻译接口。

```sh
python3 tools/rust_migration/cargo.py test
macos/.venv/bin/python -m unittest discover -s core/python -p 'test_*.py'
```

首次运行 Cargo 前，先构建与稳定 Python 环境相同的原生 OpenCV 4.11.0；构建到工作目录，不安装到系统：

```sh
python3 -m venv .runtime/build-tools
.runtime/build-tools/bin/python -m pip install cmake==3.31.6 ninja==1.11.1.4
python3 tools/rust_migration/build_opencv.py \
  --directory .runtime/native-deps \
  --cmake .runtime/build-tools/bin/cmake \
  --ninja .runtime/build-tools/bin/ninja
python3 tools/rust_migration/cargo.py build
```

## 模型边界

- Rust 创建、串行使用和关闭模型子进程，限制请求超时；取消或超时后不能复用可能含旧响应的通道。
- 标准输入/输出使用带请求编号的 JSONL；诊断只写标准错误。
- 图像和蒙版通过任务目录中的 uint8 二进制数组传递，附带形状；避免重复 PNG/base64 转换。
- Python 模型延迟加载并在同一进程内复用，不自动下载模型，不保留任务/翻译缓存。
- 输出路径必须位于任务目录，禁止覆盖已有数组；原图和原始译文不是输出目标。

`model-health` 可验证握手和退出，`model-run` 用于批量验证真实模型输入输出。完整 `display` 流程经由 `model_images.rs` 交换受限原始数组，Python 只处理模型输入/输出。

`image-probe` 检查原生解码像素，`panel-probe` 和 `structure-probe` 运行 Rust 分镜、对话分组、阅读顺序和译文同步。输入缓存只读，结果输出到标准输出。`structure-probe --mode build` 强制重建，`--mode sync` 仅同步译文，默认 `ensure` 保留缓存失效规则。

`ink-probe` 核对显示框、页码保留和独立长划线补全后的蒙版哈希。它使用已有检测蒙版，不加载模型。

`geometry-probe` 核对气泡/旁白轮廓、排版矩形、擦除多边形及旧缓存边界。`background-probe` 接收一组局部修补请求，核对清字计划及蒙版，不运行修补模型。两者的差分验证脚本分别是 `verify_geometry.py` 和 `verify_background_plan.py`；原始图片及缓存保持只读。

## 完整显示生成

构建 release 后运行（输入路径由调用方指定，输出 JSON 写标准输出）：

```sh
python3 tools/rust_migration/cargo.py build --release
core/target/release/koma-core display --image /path/to/page.webp --cache /path/to/page.json \
  --python macos/.venv/bin/python --worker core/python/model_worker.py --weights .models
```

三个模型参数必须一起提供。省略时走原生非模型修补路径；`--text-mask /path/to/mask.png` 可提供已有检测蒙版。输出兼容现有显示协议 version 3，包含 `regions` 和 `preserved`。每次调用只读原图/译文，模型数组放在独立临时目录。

`background-render` 用于局部修补差分验证。`verify_display.py` 比较完整区域与修补像素，`verify_rendered_pages.py` 用现有 Swift 离线渲染器比较整页像素、字形和译文完整性，`verify_reader_behavior.py` 将既有阅读显示行为断言接到 Rust CLI。

实际 OCR 页面组装、持久显示缓存及常驻任务服务已实现；候选应用主要调用已接入，剩余服务生命周期迁移、真实界面验收和最终对照报告仍未完成。

## 常驻应用服务

`serve --cache-root <candidate-pages> --assets-root <candidate-assets> --seed-root <stable-pages> --python <python> --worker core/python/model_worker.py --weights <weights>` 使用标准输入/输出 JSONL。每条请求具有唯一字符串 `id` 和 `op`，支持 `health`、`load_page`、`process`、`display`、`edit`、`models`、`cancel`、`shutdown`。凭据仅在请求内传递，不放进命令行、响应或推理子进程环境。

Rust 串行调度跨页处理任务；显示请求单独限流，共用延迟启动的模型进程。取消后销毁并等待受中断的模型进程退出，下一次推理使用新进程和临时目录；标准输入关闭会取消任务并清理子进程。非终态进度事件在客户端过慢时可合并丢弃，终态和每页完成事件可靠发送。调用方必须持续读取输出。

候选页面和显示缓存目录不得与只读种子缓存重叠。首次打开按图像哈希复制已有页面，原文、译文和未知元数据保留。编辑支持版本校验，拒绝过期修改及越界写入。显示缓存含代码、输入和模型文件身份以及载荷校验，损坏时重新生成。

`verify_runtime.py --manifest <pages.json> --output <new-directory> --display --baseline-results <native-display-01>` 验证真实常驻服务的队列、取消、崩溃恢复、手改保护和整章显示缓存；故障测试使用模拟推理与回环 HTTP，整章显示使用已安装的真实检测/修补模型。`verify_ocr.py` 单独比较旧版与原生流程的真实 OCR 和完整页面装配。

候选应用由 `macos/scripts/build.sh` 构建为 `Koma Rust 候选版.app`，标识 `local.koma.reader.rust`。默认数据位于 `~/Library/Application Support/KomaReaderRust`，首次配置只读迁移稳定版配置到独立副本，不访问钥匙串。`NativeCoreService.swift` 持有 Rust 服务的管道，持续读取并匹配请求编号；Swift 保留界面和排版。当前本地 Ollama 自动启动/停止仍在 Swift，需迁移后才能认定职责切换完整。

## 普通翻译与 OCR 区域分类

`overlay-probe --cache /path/to/page.json --image /path/to/page.webp` 只读输入并返回旧 OCR 区域协议及缓存变化标记；它与最终显示区域生成是不同阶段。

`translate` 从已有 OCR 页面开始，用 Rust 完成翻译请求、响应校验、普通翻译批处理和逐批保存。此命令会向配置的模型发出真实生成请求；`list-models` 仅查询模型元数据。认证读取进程环境变量 `KOMA_TRANSLATION_API_KEY`，不会写入页面缓存或指标，也不会传给 Python 模型进程。应用配置接入仍待完成。

```sh
core/target/release/koma-core translate \
  --cache /path/to/input-page.json --destination /path/to/candidate-page.json \
  --image /path/to/page.webp --status /path/to/candidate-status.json \
  --provider omlx --base-url http://127.0.0.1:8000/v1 --model installed-model \
  --unit blocks --batch-size 8
```

`--unit dialogues` 使用完整对白；`--block-id` 仅处理指定块，保留手工修改；`--metrics` 可选输出只含已知计时/计数字段的 JSONL。目标页面原子替换、权限 0600。部分翻译失败保存在对应文字块的 `error` 中，命令成功结束不等于所有句子都翻译成功。

`verify_translation_protocol.py` 对照提示词、解析规则和本地模拟 HTTP；`verify_overlay.py` 对照 OCR 区域分类；`verify_translate_pipeline.py` 对照每批请求、页面与保存快照。这些脚本只使用模拟模型回复，不访问已有 oMLX/Ollama 服务。`protocol-probe`、`service-probe`、`translation-probe` 是开发验证入口，不能代表候选应用已接入。

## 视觉翻译

给上述 `translate` 命令加 `--vision` 即使用视觉流程；必须提供 `--image`，服务须为 oMLX。此模式使用固定 8 块一批及最长边 1775 像素的完整页 JPEG，不使用普通模式的对白单位和批大小选项。生成的识别修正建议保存在 `vision_source`，原 `source` 不自动替换。

`vision-image --image /path/to/page.webp` 只生成模型输入图片，返回包含 JPEG data URL 和尺寸的 JSON，不调用任何模型。解码、EXIF 方向、颜色转换、缩放和 JPEG 编码均在原生核心内执行。Rust 缩放规则对照 Pillow，许可与来源见 [第三方说明](THIRD_PARTY.md)。CMYK 解码通过一个受限 C ABI 桥调用与 OpenCV 相同的 libjpeg，桥内没有业务流程。`core/build.rs` 使用任务目录里的头文件和静态库；自定义依赖根目录时，可对 `cargo.py` 设置 `KOMA_NATIVE_DEPS`。

`verify_vision_image.py` 对照 35 页和格式/方向/大图案例的 JPEG 字节；`verify_vision_pipeline.py` 以本地模拟服务对照完整图片请求、源文保护、建议、缓存初始化和保存快照。`vision-probe` 仅作为这些开发验证的传输入口。

差分检查使用稳定版本作为测试参照，生产核心不调用参照程序：

```sh
macos/.venv/bin/python tools/rust_migration/verify_structure.py \
  --baseline-root ../screen-capture-translator-public \
  --manifest .runtime/rust-migration/pages.json \
  --output .runtime/rust-migration/structure-review
```

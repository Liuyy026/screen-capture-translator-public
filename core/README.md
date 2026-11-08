# Koma Rust core（重构进行中）

本目录是独立分支的实验核心，尚不能替代稳定阅读器。目标职责与验收标准见 [迁移记录](../docs/rust-core-migration.md)。Python 模型进程仅运行检测、OCR 和背景修补模型；不能依赖旧 Python 业务流程完成所谓 Rust 处理。

## 本地开发

使用 Rust 工具链以及现有、已安装模型的 Python 环境。无需重新下载模型，不读取 API Key，不访问翻译接口。

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

目前 `model-health` 可验证握手和退出，`model-run` 用于批量验证真实模型输入输出。它们是开发检查接口，不代表显示和翻译流程已完成 Rust 迁移。

`image-probe` 检查原生解码像素，`panel-probe` 和 `structure-probe` 运行 Rust 分镜、对话分组、阅读顺序和译文同步。输入缓存只读，结果输出到标准输出。`structure-probe --mode build` 强制重建，`--mode sync` 仅同步译文，默认 `ensure` 保留缓存失效规则。

差分检查使用稳定版本作为测试参照，生产核心不调用参照程序：

```sh
macos/.venv/bin/python tools/rust_migration/verify_structure.py \
  --baseline-root ../screen-capture-translator-public \
  --manifest .runtime/rust-migration/pages.json \
  --output .runtime/rust-migration/structure-review
```

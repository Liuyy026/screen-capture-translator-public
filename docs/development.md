# 共用后端与评估

[项目首页](../README.md)

以下命令均在仓库根目录运行，使用已安装项目依赖的 Python 环境。各层级名为 `test/` 或 `tests/` 的目录仅供本地使用，由 `.gitignore` 排除，请勿提交。

## 核心回归测试

公开检出可运行 `core/python/` 下的模型进程协议和数组边界测试，素材自动生成，无需真实模型服务。

```sh
python scripts/check_regressions.py
```

默认入口也会运行已经存在的本地 Python 套件。添加 `--local` 后，必须同时提供 `tests/` 的共用后端用例和 `macos/tests/` 的 macOS 基础用例。

```sh
python scripts/check_regressions.py --local
```

| 选项 | 所需本地用例与依赖 |
| --- | --- |
| `--native` | `tests/`、`macos/tests/` 中供 `tools/rust_migration/verify_*.py` 复用的 Python 断言，以及已构建的 Rust 核心 |
| `--swift` | `macos/tests/*Tests.swift`、macOS 和 Xcode 命令行工具 |
| `--windows` | `windows/tests/` 中的 Python 用例和 PySide6 |

准备好本地用例与原生依赖后，可构建 Rust 核心并运行下列检查。

```sh
python3 tools/rust_migration/cargo.py test -- --test-threads=1
python3 tools/rust_migration/cargo.py build
python scripts/check_regressions.py --native --swift
```

`--native` 默认使用 `core/target/debug/koma-core`，可用 `KOMA_TEST_CORE` 指向另一个与当前源码一致的已构建程序。请求的套件缺少本地用例或依赖时会报错。统一入口退出时会清理本次在 `.runtime/` 中的临时输出。单独运行验证器时，可指定新的 `--output` 目录保留报告。

本地图片、模型权重、运行缓存和覆盖率报告也请勿提交。实际模型质量、真实 GPU 推理和完整窗口人工体验需要另行验证。

## 历史验收与评测

2026-11-14 精简了旧模型/A-B/标注评测工具、35 页迁移对照、截图验收以及依赖历史素材的专项脚本。相关历史源码可从 [精简前版本](https://github.com/Liuyy026/screen-capture-translator-public/tree/75c7276a93f1d6a55ff09592cf1b2dbd3aede91f) 恢复，结果记录仍保留在 `docs/` 和 `进度汇总/`；旧报告中的命令只适用于对应历史版本，不能视为当前测试入口或本轮验证结果。

## 本地模型服务

macOS 和 Windows 都使用本机 `127.0.0.1:11439`，并关闭云端功能。模型文件放在项目 `.models\ollama`（macOS 为 `.models/ollama`），不纳入 Git。Windows 手动启动服务时：

```powershell
$env:OLLAMA_HOST = '127.0.0.1:11439'
$env:OLLAMA_MODELS = (Resolve-Path '.models\ollama').Path
$env:OLLAMA_NO_CLOUD = '1'
$env:OLLAMA_MAX_LOADED_MODELS = '1'
$env:OLLAMA_NUM_PARALLEL = '1'
ollama serve
```

检查已安装模型和 GPU 占用：

```powershell
$env:OLLAMA_HOST = '127.0.0.1:11439'
ollama list
ollama ps
```

当前 Windows 默认使用已验证的 `qwen3:4b-q6k`（Q6_K，约 3.3 GB）；`qwen3:4b` 和 `qwen3:8b` 仍可在界面中切换。官方没有 `qwen3:9b` 标签。完整翻译流程需要 Ollama 保持在上述端口运行。原 `macos/scripts/setup.py` 下载的是 macOS 二进制，不能用于 Windows 完整安装。

网络可用时下载 OCR 模型：

```powershell
.\windows\.venv\Scripts\python.exe -X utf8 scripts\prepare_models.py
```

页面缓存按图片内容 SHA-256 命名，原图保持只读。客户端日志与系统缓存位置见各平台说明。共用评估脚本仍可使用根目录 `.runtime/`。Linux 后端缓存使用 `$XDG_DATA_HOME/KomaReader/pages/`，未设置时为 `~/.local/share/KomaReader/pages/`。每页在独立 Python 进程中处理；检测在 CPU 运行，Manga OCR 使用可用的 MPS/CUDA。

[路径迁移与本次验证](layout-migration.md)

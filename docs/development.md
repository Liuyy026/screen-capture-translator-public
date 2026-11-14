# 共用后端与评估

[项目首页](../README.md)

以下命令均在仓库根目录运行，使用所选平台的 Python 环境。`backend/`、`scripts/`、`tests/`、`samples/` 为共用内容；Windows 界面测试单独放在 `windows/tests/`。

## 核心回归测试

测试保留翻译协议与失败恢复、手改保护、分镜/对白、清字与排版、缓存边界、任务取消和两端界面行为。测试素材自动生成；不需要私人漫画、历史 manifest 或真实模型服务。

| 范围 | 入口 |
| --- | --- |
| 共用后端 | `tests/test_worker.py`、`test_translation_service.py`、`test_panels.py`、`test_structure.py`、`test_overlay.py` |
| macOS 显示与视觉输入 | `macos/tests/test_*.py` |
| 模型进程协议与数组边界 | `core/python/test_model_worker.py` |
| Rust 协议、普通/视觉翻译、显示、队列与取消 | `tools/rust_migration/verify_*.py`，复用当前 Python 行为断言和合成素材 |
| Rust 缓存与原生底层 | `core/src/` 内的单元测试 |
| Swift 配置、拖动、排版与进程通信 | `macos/tests/*Tests.swift` |
| Windows Qt 界面与服务选择 | `windows/tests/` |

使用已安装项目依赖的 Python，在仓库根目录运行：

```sh
python scripts/check_regressions.py
```

原生依赖准备好后构建并运行 Rust 测试，再启用 Rust/Swift 回归：

```sh
python3 tools/rust_migration/cargo.py test -- --test-threads=1
python3 tools/rust_migration/cargo.py build
python scripts/check_regressions.py --native --swift
```

`--native` 默认使用 `core/target/debug/koma-core`，可用 `KOMA_TEST_CORE` 指向另一个与当前源码一致的已构建程序；`--swift` 需要 macOS/Xcode 命令行工具。安装 PySide6 后可添加 `--windows` 运行 Qt 测试。请求的测试缺少依赖时会报错，不会伪装成跳过后通过。临时输出在 `.runtime/` 中并在统一入口退出时清理；单独运行验证器可指定新的 `--output` 目录以保留报告。

测试源码随 Git 提交；模型、运行缓存、覆盖率报告与本地图片不提交。`.gitignore` 保留相应排除规则。实际模型质量、真实 GPU 推理和完整窗口人工体验不属于这套离线核心回归。

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

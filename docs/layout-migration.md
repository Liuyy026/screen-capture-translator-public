# 平台目录整理 / Platform layout migration

日期：2026-11-07。macOS 与 Windows 专用内容分目录维护，共享后端保留一份。保留完整仓库，各平台目录不能单独复制运行。

## 新旧路径 / Path mapping

| 原路径 / Previous | 新路径 / Current |
| --- | --- |
| `Sources/` | `macos/Sources/` |
| `安装.command` | `macos/安装.command` |
| `启动.command` | `macos/启动.command` |
| `scripts/setup.py` | `macos/scripts/setup.py` |
| `scripts/build.sh` | `macos/scripts/build.sh` |
| `requirements.txt`、`requirements.lock` | `macos/requirements.txt`、`macos/requirements.lock` |
| `windows_app.py` | `windows/windows_app.py` |
| `启动Windows.bat` | `windows/启动Windows.bat` |
| `requirements-windows.txt` | `windows/requirements.txt` |
| `requirements-windows.lock` | `windows/requirements.lock` |
| `requirements-windows-cuda.txt` | `windows/requirements-cuda.txt` |
| `scripts/migrate_ollama_models.ps1` | `windows/scripts/migrate_ollama_models.ps1` |
| `tests/test_canvas.py` | `windows/tests/test_canvas.py` |

`backend/`、共用 `scripts/`、`tests/fixtures/`、`samples/`、`docs/` 和 `进度汇总/` 保留在根目录。

## 运行数据 / Runtime data

| 内容 | macOS | Windows |
| --- | --- | --- |
| Python 环境 | `macos/.venv/` | `windows/.venv/`、`windows/.venv-cuda/` |
| 客户端运行文件与日志 | `macos/.runtime/` | `windows/.runtime/` |
| 应用构建产物 | `macos/dist/`、`macos/.build/` | 当前使用 Python 界面 |
| OCR 与 Ollama 模型 | 根目录 `.models/` | 根目录 `.models/`，支持启动器环境变量覆盖 Ollama 仓库 |

页面缓存仍位于操作系统用户数据目录，图片内容哈希和缓存格式未变。共用评估脚本仍可向根目录 `.runtime/` 写入评测结果。Git 忽略规则适用于这些目录的各层级。

本次从干净克隆开始，没有既有运行环境、模型或用户缓存需要移动。旧副本应按平台说明重新建立虚拟环境；虚拟环境包含绝对路径，不建议直接复制。macOS 应用保存构建时的仓库位置，移动整个仓库后需要重新构建。

历史进度文档保持原文，其中旧代码位置按上表换算。旧测试命令只覆盖根目录 `tests/`；现在 Windows 开发需要额外运行 `python -m unittest discover -s windows/tests -v`。所有开发命令均从仓库根目录运行。

Shared source, model storage, system page caches, and backend CLI behavior remain at their previous locations. Environments, client logs, and build products now live under each platform directory. Recreate environments and rebuild the macOS app after moving the repository. Historical documentation retains its original paths; use the table above. Run both shared tests and `windows/tests/` when developing the Windows client.

## 目录整理阶段验证（安装前） / Initial verification

- 共用后端：64 项测试运行，62 项通过，2 项因原始漫画素材未提供而跳过。
- Windows 既有界面测试：在 macOS 的离屏 Qt 环境中运行 29 项，27 项通过；2 项字体/像素断言失败，原始提交复测出现完全相同的失败。尚未在真实 Windows 上运行启动器和 CUDA。
- 新增 2 项路径回归测试全部通过：从仓库外直接加载 Windows 界面可找到共用后端；工作进程使用共用脚本与 Windows 专用状态目录，界面能读取其进度。
- macOS 实际构建：当前 Command Line Tools 缺少 `SwiftUIMacros.StateMacro` 插件，原始 Swift 源码同样失败；本次不能声明已生成可运行应用。
- 未下载 OCR/翻译模型，未启动本地模型服务，未重跑真实翻译或 GPU 评测。

这些是目录整理的检查记录，不能替代真实平台启动、OCR、语义与排版验收。Dependency pins and model settings are retained.

## 后续 macOS 安装（2026-11-07）

上面的 macOS 编译阻碍已在后续任务中解决，并完成依赖、模型安装、界面启动和真实样例翻译。当前状态以[macOS 验证报告](macos-verification-2026-11-07.md)为准；本节保留早期整理时的验证记录。

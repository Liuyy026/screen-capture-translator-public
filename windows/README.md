# Koma · Windows

简体中文 | [English](README.en.md) | [项目首页](../README.md)

请保留完整仓库。以下命令均在仓库根目录执行。环境分别位于 `windows/.venv`、`windows/.venv-cuda`，后端位于根目录 `backend/`，模型保留在根目录 `.models/`。

## Windows 启动

双击本目录的 **启动Windows.bat**。启动器会按顺序检查 `.venv-cuda` 和 `.venv`，选择第一个能加载 PySide6 的 Python；OCR 子进程沿用相同解释器。它会在 `11439` 端口未监听时尝试启动本机 Ollama，并通过系统级单实例锁避免重复打开窗口。

模型仓库可以集中放在系统 Ollama 默认目录，同时保留项目路径兼容现有启动流程。先退出翻译软件和 Ollama，预览迁移计划：

```powershell
powershell -ExecutionPolicy Bypass -File .\windows\scripts\migrate_ollama_models.ps1
```

确认目标目录为空且路径正确后，才执行实际迁移：

```powershell
powershell -ExecutionPolicy Bypass -File .\windows\scripts\migrate_ollama_models.ps1 -Apply
```

脚本会移动完整的 `blobs` 和 `manifests`，然后在原 `.models\ollama` 路径建立 Windows junction。启动器仍默认使用原路径，因此翻译软件不需要改用另一套模型仓库；也可以通过 `KOMA_OLLAMA_MODELS` 指向其他完整仓库。迁移前必须关闭监听 `11434` 或 `11439` 的 Ollama 实例，目标目录必须为空。不要只移动大文件，Ollama 的 manifests 与 blobs 必须一起迁移。

启动过程和错误记录在 `windows\.runtime\windows-launch.log`。只检查环境、不启动界面时运行：

```powershell
.\windows\启动Windows.bat --check
```

启动成功后会看到 **Koma · 本地漫画翻译** 窗口。最小验证流程：打开项目内 `samples` 文件夹，选中 `01.webp`，点击「识别并翻译」，等待状态变为「完成」，右侧应显示 5 条日文及其中文译文。

如果双击后没有窗口，先查看 `windows\.runtime\windows-launch.log`。启动器会区分 Python 运行时失败、PySide6 加载失败和应用自身错误。

## Windows 环境复现

Windows 开发环境使用 Python 3.12.14 验证，系统中的其他 Python 版本可以保留。无需激活虚拟环境，直接指定解释器即可：

```powershell
.\windows\.venv\Scripts\python.exe --version
.\windows\.venv\Scripts\python.exe -X utf8 -m unittest discover -s tests -v
.\windows\.venv\Scripts\python.exe -X utf8 backend\worker.py --help
```

如需激活环境，可在 PowerShell 中执行：

```powershell
.\windows\.venv\Scripts\Activate.ps1
$env:PYTHONUTF8 = '1'
```

如果执行策略阻止激活，继续使用上面的完整解释器路径即可。

Windows 后端读写中文 JSON 时建议使用上面的 `-X utf8` 参数，或设置 `PYTHONUTF8=1`。

IDE 的 Python 解释器请选择 `windows\.venv\Scripts\python.exe`。本机完整 Python 运行时放在 `windows\.runtime\python`；如果移动整个项目目录，需要同步更新两个虚拟环境中的 `pyvenv.cfg`，使其 `home` 指向新的运行时位置。

在另一台已有 Python 3.12 的 Windows 机器上，可从项目目录重新创建基础环境：

```powershell
py -3.12 -m venv windows\.venv
.\windows\.venv\Scripts\python.exe -X utf8 -m pip install -r windows/requirements.lock
```

也可以使用项目内的 uv：

```powershell
.\windows\.runtime\tools\uv.exe pip install --python windows\.venv\Scripts\python.exe -r windows/requirements.lock
```

`windows/requirements.txt` 是 Windows 基础依赖，`windows/requirements.lock` 记录本次验证过的完整依赖。整理前的 macOS 锁文件使用 `transformers==4.44.2`，与 Manga OCR 0.1.16 不兼容；后续 macOS 安装也已修正为 4.45.2。两个平台分别维护依赖，Windows 使用 `transformers==4.45.2`。Windows 依赖文件不要直接用于 macOS。

### CUDA 环境

有 NVIDIA GPU 时，Windows 阅读器优先使用 `.venv-cuda`。当前验证环境使用 RTX 5070、CUDA 12.8：

```powershell
.\windows\.venv\Scripts\python.exe -m venv windows\.venv-cuda
.\windows\.venv-cuda\Scripts\python.exe -m pip install --index-url https://download.pytorch.org/whl/cu128 torch==2.7.1 torchvision==0.22.1
.\windows\.venv-cuda\Scripts\python.exe -m pip install -r windows/requirements-cuda.txt
.\windows\.venv-cuda\Scripts\python.exe -m pip install PySide6==6.9.2
```

没有 NVIDIA GPU 时可只使用 `.venv`；程序会自动回退到 CPU 或系统可用的加速后端。


## 模型与开发验证

需另行安装 Windows 版 Ollama。默认模型仍为 `qwen3:4b-q6k`；启动器依次尝试 `KOMA_OLLAMA_MODELS`、`OLLAMA_MODELS`，否则使用仓库根目录 `.models/ollama`。迁移脚本默认也是该目录。Ollama 使用 `127.0.0.1:11439`，关闭云端功能。以下命令在仓库根目录执行，下载 OCR 模型需要联网：

```powershell
.\windows\.venv\Scripts\python.exe -X utf8 scripts\prepare_models.py
.\windows\.venv\Scripts\python.exe -X utf8 -m unittest discover -s tests -v
.\windows\.venv\Scripts\python.exe -X utf8 -m unittest discover -s windows/tests -v
.\windows\.venv\Scripts\python.exe -X utf8 backend\worker.py --image samples\01.webp --operation both --status windows\.runtime\test-status.json
```

使用 CUDA 时将 `.venv` 换成 `.venv-cuda`。共用测试 `tests/` 与界面测试 `windows/tests/` 两组都要运行。界面入口为 `windows/windows_app.py`；结果区仍为只读，编辑、重译和文件夹队列尚未接入。启动日志和进度文件在 `windows/.runtime/`，页面缓存仍在 `%LOCALAPPDATA%\KomaReader\pages\`。

[共用开发说明](../docs/development.md) · [旧路径对照](../docs/layout-migration.md)

## 翻译服务：Ollama / oMLX

在阅读器中打开接口/服务设置，选择服务，填写地址与可选 API Key，获取模型列表、选择模型后保存。oMLX 默认为 `http://127.0.0.1:8000/v1`，Ollama 默认为 `http://127.0.0.1:11439`。oMLX 需自行运行，应用不会启停它。

Windows API Key 仅在当前运行中保留，重开后需重新填写。地址和模型选择会保存；切换服务后重新生成机器译文，手改译文继续保留。操作、CLI 参数和真实样例记录见[接入与验证说明](../docs/omlx-integration-2026-11-07.md)。

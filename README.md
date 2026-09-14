# Koma · 本地漫画翻译

Koma 是一个本地运行的漫画翻译工具：打开包含图片的文件夹，自动检测漫画文字，用 Manga OCR 识别日文，再通过本机 Ollama 和 Qwen3 翻译成简体中文。项目包含 macOS 原生 SwiftUI 阅读器和 Windows PySide6 验证版，共用同一套 Python 后端。

## 功能

- 支持 WebP、PNG、JPG、JPEG、BMP、TIFF，按文件名自然排序。
- 自动识别横排和竖排文字，显示文字框、原文和中文覆盖。
- 可在右侧修改日文或中文；手动保存的中文不会被批量翻译覆盖。
- 支持单句重新翻译、整文件夹批处理、取消任务和缓存复用。
- 原图保持只读，识别结果、译文、错误状态和覆盖区域写入本地缓存。

## macOS 启动

项目针对 Apple Silicon、macOS 14+ 开发。完成安装后，双击根目录的 **启动.command**，或打开 **dist/Koma 漫画翻译.app**。

1. 点击「打开文件夹」，选择直接包含图片的文件夹。
2. 点击「识别并翻译」。也可以通过「更多」菜单只识别日文，或翻译整个文件夹。
3. 切换「原图 / 文字框 / 中文」查看不同显示模式。
4. 在右侧编辑文本，点击「保存修改」；修改日文会清除对应旧译文。
5. 重新翻译单句前先保存编辑，再点击该句下方的「重新翻译」。

`samples/` 中的两张原创 WebP 测试图包含横排和竖排日文，可用于检查流程，不代表真实漫画上的识别准确率。

## Windows 启动

Windows 阅读器直接双击根目录的 **启动Windows.bat**。启动器会按顺序检查 `.venv-cuda` 和 `.venv`，选择第一个能加载 PySide6 的 Python；OCR 子进程沿用相同解释器。它会在 `11439` 端口未监听时尝试启动本机 Ollama，并通过系统级单实例锁避免重复打开窗口。

启动过程和错误记录在 `.runtime\windows-launch.log`。只检查环境、不启动界面时运行：

```powershell
.\启动Windows.bat --check
```

启动成功后会看到 **Koma 漫画翻译 - Windows 验证版**。最小验证流程：打开项目内 `samples` 文件夹，选中 `01.webp`，点击「识别并翻译」，等待状态变为「完成」，右侧应显示 5 条日文及其中文译文。

如果双击后没有窗口，先查看 `.runtime\windows-launch.log`。启动器会区分 Python 运行时失败、PySide6 加载失败和应用自身错误。

## Windows 环境复现

Windows 开发环境使用 Python 3.12.14 验证，系统中的其他 Python 版本可以保留。无需激活虚拟环境，直接指定解释器即可：

```powershell
.\.venv\Scripts\python.exe --version
.\.venv\Scripts\python.exe -X utf8 -m unittest discover -s tests -v
.\.venv\Scripts\python.exe -X utf8 backend\worker.py --help
```

如需激活环境，可在 PowerShell 中执行：

```powershell
.\.venv\Scripts\Activate.ps1
$env:PYTHONUTF8 = '1'
```

如果执行策略阻止激活，继续使用上面的完整解释器路径即可。

Windows 后端读写中文 JSON 时建议使用上面的 `-X utf8` 参数，或设置 `PYTHONUTF8=1`。

IDE 的 Python 解释器请选择 `.venv\Scripts\python.exe`。本机完整 Python 运行时放在 `.runtime\python`；如果移动整个项目目录，需要同步更新两个虚拟环境中的 `pyvenv.cfg`，使其 `home` 指向新的运行时位置。

在另一台已有 Python 3.12 的 Windows 机器上，可从项目目录重新创建基础环境：

```powershell
py -3.12 -m venv .venv
.\.venv\Scripts\python.exe -X utf8 -m pip install -r requirements-windows.lock
```

也可以使用项目内的 uv：

```powershell
.\.runtime\tools\uv.exe pip install --python .venv\Scripts\python.exe -r requirements-windows.lock
```

`requirements-windows.txt` 是 Windows 基础依赖，`requirements-windows.lock` 记录本次验证过的完整依赖。原 `requirements.lock` 中的 `transformers==4.44.2` 与 Manga OCR 0.1.16 不兼容，因此 Windows 使用 `transformers==4.45.2`。Windows 依赖文件不要直接用于 macOS。

### CUDA 环境

有 NVIDIA GPU 时，Windows 阅读器优先使用 `.venv-cuda`。当前验证环境使用 RTX 5070、CUDA 12.8：

```powershell
.\.venv\Scripts\python.exe -m venv .venv-cuda
.\.venv-cuda\Scripts\python.exe -m pip install --index-url https://download.pytorch.org/whl/cu128 torch==2.7.1 torchvision==0.22.1
.\.venv-cuda\Scripts\python.exe -m pip install -r requirements-windows-cuda.txt
.\.venv-cuda\Scripts\python.exe -m pip install PySide6==6.9.2
```

没有 NVIDIA GPU 时可只使用 `.venv`；程序会自动回退到 CPU 或系统可用的加速后端。

## Ollama 与模型

macOS 和 Windows 都使用本机 `127.0.0.1:11439`，并关闭云端功能。模型文件放在项目 `.models\ollama`（macOS 为 `.models/ollama`），不纳入 Git。Windows 手动启动服务时：

```powershell
$env:OLLAMA_HOST = '127.0.0.1:11439'
$env:OLLAMA_MODELS = (Resolve-Path '.models\ollama').Path
$env:OLLAMA_NO_CLOUD = '1'
ollama serve
```

检查已安装模型和 GPU 占用：

```powershell
$env:OLLAMA_HOST = '127.0.0.1:11439'
ollama list
ollama ps
```

当前 Windows 验证过 `qwen3:4b`（约 2.5 GB）和 `qwen3:8b`（约 5.2 GB）；官方没有 `qwen3:9b` 标签。完整翻译流程需要 Ollama 保持在上述端口运行。原 `scripts/setup.py` 下载的是 macOS 二进制，不能用于 Windows 完整安装。

网络可用时下载 OCR 模型：

```powershell
.\.venv\Scripts\python.exe -X utf8 scripts\prepare_models.py
```

## 数据、缓存与日志

- macOS 缓存：`~/Library/Application Support/KomaReader/pages/`。
- Windows 缓存：`%LOCALAPPDATA%\KomaReader\pages\`。
- Linux 缓存：`$XDG_DATA_HOME/KomaReader/pages/`，未设置时使用 `~/.local/share/KomaReader/pages/`。
- 缓存按图片内容 SHA-256 命名；文件重命名仍可命中，同名但内容不同的图片不会串页。
- OCR 日志：`.runtime/ocr.log`；模型下载日志：`.runtime/model-download.log`；Windows 启动日志：`.runtime\windows-launch.log`。
- `.venv`、`.venv-cuda`、`.runtime`、`.models`、构建产物和 Python 缓存均不应提交到 Git。

## 开发与验证

macOS：

```sh
zsh scripts/build.sh
.venv/bin/python -m unittest discover -s tests -v
.venv/bin/python backend/worker.py --image samples/01.webp --operation both --status .runtime/test-status.json
```

Windows：

```powershell
.\.venv\Scripts\python.exe -X utf8 -m unittest discover -s tests -v
.\.venv\Scripts\python.exe -X utf8 backend\worker.py --image samples\01.webp --operation both --status .runtime\test-status.json
```

实现位置：

- `Sources/KomaReader.swift`：macOS 原生窗口、文件夹选择、阅读、覆盖和编辑。
- `windows_app.py`：Windows PySide6 阅读器、缩放、覆盖和单实例控制。
- `backend/worker.py`：单页 OCR、缓存、翻译、编号校验和错误保存。
- `backend/overlay.py`：文字区域分析和中文覆盖几何计算。
- `scripts/build.sh`：使用系统 Swift 编译器生成并本地签名 macOS App。
- `启动Windows.bat`：Windows Python、Ollama 和日志启动器。
- `tests/`：缓存隔离、覆盖区域、漏译/重复编号、手动编辑保留等后端测试。

每页在独立 Python 进程中处理，页面完成后释放 OCR 内存；未缓存页面需要重新加载模型。文字检测在 CPU 运行，Manga OCR 会使用本机可用的 MPS 或 CUDA 加速。翻译模型从 4B 起步，可按机器资源切换其他本地模型。

## 当前验证结果与限制

- macOS 原生应用已编译并本地签名；Windows 阅读器可通过启动器运行。
- Windows 基础环境已通过 `pip check`，当前项目 51 项 Python 单元测试通过。
- 两张自制 WebP 测试页已完成 OCR → Qwen3 4B 翻译，横排和竖排流程均通过。
- Windows RTX 5070 环境中 Manga OCR 日志显示 `Using CUDA`，Ollama 模型进程显示 GPU 使用；`samples/01.webp` 已识别出 5 个文字框，4B 和 8B 均返回 5 条有效译文。
- 输出按本次真实编号校验；占位文字、空译文、重复编号和漏译会明确标错，旧缓存和手动编辑会被保留。
- 尚未用真实漫画系统评估。竖排、手写、拟声词、低清图片及跨分镜阅读顺序可能出错。
- 中文模式是白底覆盖，不是无痕擦字或专业汉化排版；暂不支持补框/拆框、跨页人物记忆、术语表、导出汉化图片、ZIP/PDF 和子文件夹递归。
- 本地模型可能拒译、误译或返回格式错误；程序保留原文和错误信息，不保证即时或必然得到有效译文。

## 上游项目与许可证

- [Mokuro](https://github.com/kha-white/mokuro)：GPL-3.0，提供漫画检测和逐行 OCR 流程。
- [Manga OCR](https://github.com/kha-white/manga-ocr)：日文文字识别。
- [Comic Text Detector](https://github.com/dmMaze/comic-text-detector)：漫画文字检测。
- [Ollama](https://github.com/ollama/ollama)：本地模型服务。
- [Qwen3](https://github.com/QwenLM/Qwen3)：翻译模型系列。

模型和依赖不纳入 Git；各组件遵循其上游许可证。

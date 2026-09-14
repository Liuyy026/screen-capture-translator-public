# Windows 后端开发环境

已在本目录创建 Python 3.12.14 虚拟环境 `.venv`，安装并验证 Python 后端依赖。系统的 Python 3.14 保留不变。

代码来自 GitHub main 分支压缩包；此目录没有 `.git` 历史。

## 使用环境

在项目目录打开 PowerShell。无需激活环境，直接指定解释器即可：

```powershell
.\.venv\Scripts\python.exe --version
.\.venv\Scripts\python.exe -X utf8 -m unittest discover -s tests -v
.\.venv\Scripts\python.exe -X utf8 backend\worker.py --help
启动Windows.bat
```

如需激活，可在 PowerShell 中执行：

```powershell
.\.venv\Scripts\Activate.ps1
$env:PYTHONUTF8 = '1'
```

如果系统执行策略阻止激活，使用上面的完整解释器路径即可。Windows 上运行后端时应加 `-X utf8`，因为现有代码读写中文 JSON 时未指定编码。

IDE 的 Python 解释器请选择本项目 `.venv\Scripts\python.exe`。

Windows 阅读器直接双击根目录的 `启动Windows.bat` 启动。启动器会依次实际执行 `.venv-cuda`、`.venv` 的 Python 并加载 PySide6，使用第一个通过检查的环境；OCR 子进程沿用界面当前的解释器。它也会在 11439 端口未监听时启动本机 Ollama。程序使用系统级单实例锁，连续双击不会打开多个窗口。启动过程和真实错误记录在 `.runtime/windows-launch.log`。运行 `启动Windows.bat --check` 可只检查环境。

本机 Python 3.12.14 的完整基础运行时放在项目 `.runtime\python`，两个虚拟环境的 `pyvenv.cfg` 都指向这里。之前基础 Python 实际位于 Codex 的应用缓存中，`home` 却引用普通 `AppData\Roaming\uv` 路径，导致 Codex 内测试正常、普通桌面启动报 `No Python at ...`。不要把依赖运行时装到应用私有缓存或只在 Codex 子进程中验证。移动整个项目目录后，需要将两个 `pyvenv.cfg` 中的运行时路径更新到新位置。

启动成功后会看到标题为 **Koma 漫画翻译 - Windows 验证版** 的窗口。最小功能验证：

1. 点击「打开文件夹」，选择项目内的 `samples` 文件夹。
2. 选中 `01.webp`，点击「识别并翻译」。
3. 等待状态变为「完成」，右侧应显示 5 条日文和对应中文译文。

如果双击后窗口没有出现，先查看 `.runtime\windows-launch.log`；其中会记录实际选择的 Python 环境和启动错误。启动器也会在找不到 PySide6 时给出明确提示。

## 依赖与复现

原 `requirements.lock` 同时要求 `manga-ocr==0.1.16` 和 `transformers==4.44.2`，但 Manga OCR 0.1.16 要求 Transformers >=4.45.0，无法按原锁定文件安装。

本地使用 `requirements-windows.txt`：保留 PyTorch 2.5.1、Mokuro 0.2.0 等核心版本，使用 Transformers 4.45.2。`requirements-windows.lock` 记录本次实际安装的全部依赖；Windows 特有的依赖也包含在内，不应直接拿到 Mac 上安装。

在另一台已有 Python 3.12 的 Windows 机器上，可从项目目录执行：

```powershell
py -3.12 -m venv .venv
.\.venv\Scripts\python.exe -X utf8 -m pip install -r requirements-windows.lock
```

本机可用项目内的 uv 工具管理依赖：

```powershell
.\.runtime\tools\uv.exe pip install --python .venv\Scripts\python.exe -r requirements-windows.lock
```

`.runtime/bootstrap-py314` 是首次尝试创建的旧环境备份，当前开发环境为根目录 `.venv`。

## 验证结果与后续工作

- `pip check` 通过，10 项现有后端单元测试通过。
- Manga OCR、Mokuro、Transformers、Torch、Torchvision、OpenCV、NumPy 和 SciPy 导入成功；Torch 张量计算和 OpenCV 基础操作通过。
- 已额外创建 `.venv-cuda`，安装 `torch 2.7.1+cu128`、`torchvision 0.22.1+cu128` 及 OCR 依赖；RTX 5070 上 Manga OCR 日志显示 `Using CUDA`。Windows 阅读器会优先使用该环境。
- OCR 模型已安装：Manga OCR 主权重约 424 MiB，Comic Text Detector 权重约 76 MiB；`samples/01.webp` 已成功识别出 5 个文字框。
- Windows Ollama 0.34.0 已安装，项目服务运行在 `127.0.0.1:11439`，已下载并验证 `qwen3:4b`（约 2.5 GB）和 `qwen3:8b`（约 5.2 GB）。官方没有 `qwen3:9b` 标签，8B 是可用的近邻规格。
- 两个模型的 Ollama 进程均显示 `100% GPU`；RTX 5070 可用于翻译推理和 OCR。
- `samples/01.webp` 已完成 OCR → 翻译：4B 和 8B 均返回了 5 条有效译文，并分别保存到 `.runtime/benchmarks/` 供比较。
- 现有 SwiftUI/AppKit 界面与 `.command` 脚本仅适用于 macOS。Mac 与 Windows 可共用后端源码，但需要各自创建虚拟环境，并继续适配 Windows 界面、启动与打包流程。

网络可用后，下载 OCR 模型的命令为：

```powershell
.\.venv\Scripts\python.exe -X utf8 scripts\prepare_models.py
```

完整翻译流程需要 Windows 版 Ollama 保持在 `127.0.0.1:11439`；如果重启后端服务，可按以下方式设置环境变量后启动 `ollama serve`。原 `scripts/setup.py` 下载的是 macOS 二进制，不能直接用于 Windows 完整安装。

```powershell
$env:OLLAMA_HOST = '127.0.0.1:11439'
$env:OLLAMA_MODELS = (Resolve-Path '.models\ollama').Path
$env:OLLAMA_NO_CLOUD = '1'
ollama serve
```

如需重建 CUDA OCR 环境：

```powershell
.venv\Scripts\python.exe -m venv .venv-cuda
.venv-cuda\Scripts\python.exe -m pip install --index-url https://download.pytorch.org/whl/cu128 torch==2.7.1 torchvision==0.22.1
.venv-cuda\Scripts\python.exe -m pip install -r requirements-windows-cuda.txt
.venv-cuda\Scripts\python.exe -m pip install PySide6==6.9.2
```

当前已安装模型的检查命令：

```powershell
$env:OLLAMA_HOST = '127.0.0.1:11439'
ollama list
ollama ps
```



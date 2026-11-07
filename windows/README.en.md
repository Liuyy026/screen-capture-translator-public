# Koma · Windows

[简体中文](README.md) | English | [Project overview](../README.en.md)

Keep the full repository. Run commands from the repository root. Environments are in `windows/.venv` and `windows/.venv-cuda`; the shared backend is in `backend/`, and model weights remain in the repository-root `.models/`.

## Running on Windows

Double-click **启动Windows.bat** in this directory. The launcher checks `.venv-cuda` first, then `.venv`, and selects the first Python environment that can import PySide6. OCR workers use the same interpreter. If port `11439` is not listening, the launcher attempts to start local Ollama. A system-level single-instance lock prevents duplicate reader windows.

You can move the model store to Ollama's default user directory while keeping the project path working. First exit both the reader and Ollama, then preview the migration:

```powershell
powershell -ExecutionPolicy Bypass -File .\windows\scripts\migrate_ollama_models.ps1
```

After confirming that the destination is empty and the paths are correct, apply the migration:

```powershell
powershell -ExecutionPolicy Bypass -File .\windows\scripts\migrate_ollama_models.ps1 -Apply
```

The script moves the complete `blobs` and `manifests` store, then creates a Windows directory junction at the original `.models\ollama` path. The launcher can continue using that path. You can also set `KOMA_OLLAMA_MODELS` to another complete model store. Close Ollama instances listening on `11434` or `11439` before migration. The destination must be empty. Move manifests and blobs together, not just the large weight files.

Startup diagnostics and errors are written to `windows\.runtime\windows-launch.log`. To check the environment without opening the reader:

```powershell
.\windows\启动Windows.bat --check
```

The reader window is titled **Koma · 本地漫画翻译**. For a basic check, open the project's `samples` folder, select `01.webp`, and click **识别并翻译** (Recognize and translate). When the status becomes **完成** (Done), the right pane should show five Japanese text blocks and their Chinese translations.

If no window appears, inspect `windows\.runtime\windows-launch.log`. The launcher distinguishes Python runtime failures, PySide6 import failures, and application errors.

## Reproducing the Windows environment

The Windows development environment was validated with Python 3.12.14. Other system Python versions can remain installed. You do not need to activate the virtual environment if you use its interpreter directly. Run the following commands from the project root:

```powershell
.\windows\.venv\Scripts\python.exe --version
.\windows\.venv\Scripts\python.exe -X utf8 -m unittest discover -s tests -v
.\windows\.venv\Scripts\python.exe -X utf8 backend\worker.py --help
```

To activate it in PowerShell:

```powershell
.\windows\.venv\Scripts\Activate.ps1
$env:PYTHONUTF8 = '1'
```

If PowerShell's execution policy blocks activation, use the full interpreter path instead. On Windows, use `-X utf8` or set `PYTHONUTF8=1` when running the backend.

Select `windows\.venv\Scripts\python.exe` as your IDE's interpreter. The current local setup stores its base Python runtime in `windows\.runtime\python`. If you move the entire project, update `home` in both virtual environments' `pyvenv.cfg` files to point to the new runtime location.

On another Windows machine with Python 3.12 installed, create the base environment:

```powershell
py -3.12 -m venv windows\.venv
.\windows\.venv\Scripts\python.exe -X utf8 -m pip install -r windows/requirements.lock
```

If the project-local uv executable is available, you can also install dependencies with:

```powershell
.\windows\.runtime\tools\uv.exe pip install --python windows\.venv\Scripts\python.exe -r windows/requirements.lock
```

`windows/requirements.txt` defines the base Windows dependencies, and `windows/requirements.lock` records the validated dependency set. The original macOS lock file pinned `transformers==4.44.2`, which is incompatible with Manga OCR 0.1.16. The subsequent macOS setup also corrected this to 4.45.2. Platform requirements remain separate; Windows uses `transformers==4.45.2`. Do not use the Windows dependency files directly on macOS.

### CUDA environment

The Windows launcher prefers `.venv-cuda` when it is available. The validated GPU setup uses an RTX 5070 and CUDA 12.8:

```powershell
.\windows\.venv\Scripts\python.exe -m venv windows\.venv-cuda
.\windows\.venv-cuda\Scripts\python.exe -m pip install --index-url https://download.pytorch.org/whl/cu128 torch==2.7.1 torchvision==0.22.1
.\windows\.venv-cuda\Scripts\python.exe -m pip install -r windows/requirements-cuda.txt
.\windows\.venv-cuda\Scripts\python.exe -m pip install PySide6==6.9.2
```

Without an NVIDIA GPU, use `.venv`. OCR uses the available acceleration backend or falls back to the CPU.


## Models and development validation

Install Ollama for Windows separately. The default model remains `qwen3:4b-q6k`. The launcher prefers `KOMA_OLLAMA_MODELS`, then `OLLAMA_MODELS`, then the repository-root `.models/ollama`. The migration script defaults to the same store. Ollama uses `127.0.0.1:11439` with cloud disabled. Run from the repository root; OCR model preparation requires network access:

```powershell
.\windows\.venv\Scripts\python.exe -X utf8 scripts\prepare_models.py
.\windows\.venv\Scripts\python.exe -X utf8 -m unittest discover -s tests -v
.\windows\.venv\Scripts\python.exe -X utf8 -m unittest discover -s windows/tests -v
.\windows\.venv\Scripts\python.exe -X utf8 backend\worker.py --image samples\01.webp --operation both --status windows\.runtime\test-status.json
```

Use `.venv-cuda` as appropriate. Run both shared (`tests/`) and client (`windows/tests/`) suites. Reader: `windows/windows_app.py`. The result pane remains read-only; editing, retranslation, and folder queues are pending. Logs and progress files: `windows/.runtime/`. Page caches remain in `%LOCALAPPDATA%\KomaReader\pages\`.

[Shared development](../docs/development.en.md) · [Path mapping](../docs/layout-migration.md)

## Translation services: Ollama / oMLX

Open the service settings in the reader, choose Ollama or oMLX, enter its endpoint and optional masked API key, fetch the model list, choose a model, and save. oMLX defaults to `http://127.0.0.1:8000/v1`; Ollama defaults to `http://127.0.0.1:11439`. The application connects to an existing oMLX service and leaves its lifecycle to oMLX.

On Windows, API keys remain in session memory and must be entered again after reopening. Endpoint and model choices are saved. Switching the service invalidates machine translations while preserving manual edits. See the [usage and real-sample verification report](../docs/omlx-integration-2026-11-07.md) (Chinese) for CLI flags, results, and validation limits.

# Koma · Local Manga Translation

[简体中文](README.md) | English

Development plans, implementation notes, and evaluation summaries are available in the [progress index](进度汇总/README.md). See the [current status](进度汇总/当前进度_2026-09-14.md) for progress against the seven quality goals. These documents are currently in Chinese and are tracked in this repository; models and raw evaluation artifacts remain local.

Koma is a local manga translation tool. Open a folder of images, detect text, recognize Japanese with Manga OCR, and translate it into Simplified Chinese using a local Ollama server and Qwen3. The project includes a native SwiftUI reader for macOS and a PySide6 validation build for Windows, sharing the same Python backend.

This README provides English instructions. The application interface and translation output remain in Chinese.

## Features

- Supports WebP, PNG, JPG, JPEG, BMP, and TIFF, with natural filename sorting.
- Recognizes horizontal and vertical Japanese text and displays text boxes, source text, and Chinese overlays.
- Supports editing Japanese source text and Chinese translations; manually saved translations are preserved during batch translation.
- Supports translating an individual line again, folder batch processing, cancellation, and cache reuse.
- Keeps original images read-only. Recognition results, translations, errors, and overlay metadata are stored in a local cache.

Editing, single-line retranslation, and folder batch processing currently have UI controls in the macOS reader. The Windows validation build supports single-page processing, stopping a task, cache loading, and structure diagnostics. Its results pane is read-only; editing, retranslation, and folder queue controls are still planned.

## Running on macOS

The macOS application targets Apple Silicon and macOS 14 or later. After setup, double-click **启动.command** in the project root, or open **dist/Koma 漫画翻译.app**.

1. Click **打开文件夹** (Open folder) and select a folder containing image files directly.
2. Click **识别并翻译** (Recognize and translate). The **更多** (More) menu also offers OCR-only processing and folder translation.
3. Switch between **原图 / 文字框 / 中文** (Original / Text boxes / Chinese).
4. Edit the text in the right pane and click **保存修改** (Save changes). Changing the Japanese source clears its previous translation.
5. Save any edits before clicking **重新翻译** (Translate again) under an individual line.

The two original WebP test pages in `samples/` contain horizontal and vertical Japanese text. They verify the processing flow, not recognition accuracy on real manga.

## Running on Windows

Double-click **启动Windows.bat** in the project root. The launcher checks `.venv-cuda` first, then `.venv`, and selects the first Python environment that can import PySide6. OCR workers use the same interpreter. If port `11439` is not listening, the launcher attempts to start local Ollama. A system-level single-instance lock prevents duplicate reader windows.

You can move the model store to Ollama's default user directory while keeping the project path working. First exit both the reader and Ollama, then preview the migration:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\migrate_ollama_models.ps1
```

After confirming that the destination is empty and the paths are correct, apply the migration:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\migrate_ollama_models.ps1 -Apply
```

The script moves the complete `blobs` and `manifests` store, then creates a Windows directory junction at the original `.models\ollama` path. The launcher can continue using that path. You can also set `KOMA_OLLAMA_MODELS` to another complete model store. Close Ollama instances listening on `11434` or `11439` before migration. The destination must be empty. Move manifests and blobs together, not just the large weight files.

Startup diagnostics and errors are written to `.runtime\windows-launch.log`. To check the environment without opening the reader:

```powershell
.\启动Windows.bat --check
```

The reader window is titled **Koma · 本地漫画翻译**. For a basic check, open the project's `samples` folder, select `01.webp`, and click **识别并翻译** (Recognize and translate). When the status becomes **完成** (Done), the right pane should show five Japanese text blocks and their Chinese translations.

If no window appears, inspect `.runtime\windows-launch.log`. The launcher distinguishes Python runtime failures, PySide6 import failures, and application errors.

## Reproducing the Windows environment

The Windows development environment was validated with Python 3.12.14. Other system Python versions can remain installed. You do not need to activate the virtual environment if you use its interpreter directly. Run the following commands from the project root:

```powershell
.\.venv\Scripts\python.exe --version
.\.venv\Scripts\python.exe -X utf8 -m unittest discover -s tests -v
.\.venv\Scripts\python.exe -X utf8 backend\worker.py --help
```

To activate it in PowerShell:

```powershell
.\.venv\Scripts\Activate.ps1
$env:PYTHONUTF8 = '1'
```

If PowerShell's execution policy blocks activation, use the full interpreter path instead. On Windows, use `-X utf8` or set `PYTHONUTF8=1` when running the backend.

Select `.venv\Scripts\python.exe` as your IDE's interpreter. The current local setup stores its base Python runtime in `.runtime\python`. If you move the entire project, update `home` in both virtual environments' `pyvenv.cfg` files to point to the new runtime location.

On another Windows machine with Python 3.12 installed, create the base environment:

```powershell
py -3.12 -m venv .venv
.\.venv\Scripts\python.exe -X utf8 -m pip install -r requirements-windows.lock
```

If the project-local uv executable is available, you can also install dependencies with:

```powershell
.\.runtime\tools\uv.exe pip install --python .venv\Scripts\python.exe -r requirements-windows.lock
```

`requirements-windows.txt` defines the base Windows dependencies, and `requirements-windows.lock` records the validated dependency set. The original `requirements.lock` pins `transformers==4.44.2`, which is incompatible with Manga OCR 0.1.16; Windows uses `transformers==4.45.2`. Do not use the Windows dependency files directly on macOS.

### CUDA environment

The Windows launcher prefers `.venv-cuda` when it is available. The validated GPU setup uses an RTX 5070 and CUDA 12.8:

```powershell
.\.venv\Scripts\python.exe -m venv .venv-cuda
.\.venv-cuda\Scripts\python.exe -m pip install --index-url https://download.pytorch.org/whl/cu128 torch==2.7.1 torchvision==0.22.1
.\.venv-cuda\Scripts\python.exe -m pip install -r requirements-windows-cuda.txt
.\.venv-cuda\Scripts\python.exe -m pip install PySide6==6.9.2
```

Without an NVIDIA GPU, use `.venv`. OCR uses the available acceleration backend or falls back to the CPU.

## Ollama and models

### Dialogue units and A/B evaluation

The backend stores `dialogues`, member block IDs, reading order, and `diagnostics` in page caches. Requests can translate individual text blocks or complete dialogue units:

```powershell
python backend\worker.py --image page.webp --operation translate --translation-unit dialogues --status status.json
```

For a fixed test set, run both `--translation-unit blocks` and `--translation-unit dialogues`, then compare block and dialogue completion with `scripts/compare_translation_units.py`. Existing caches can be updated with structure metadata using `scripts/migrate_structure.py`.

To inspect structure:

```text
python scripts/evaluate_structure.py <cache-directory> --output structure-report.json
```

This evaluates migrated structure in memory, preserves source caches, and reports OCR fingerprints, membership, reading-order continuity, and review candidates. Continuous numbering is a structural consistency check, not evidence of correct human reading order.

For a complete A/B run:

```text
python scripts/run_translation_ab.py <source-cache-directory> <image-directory> <new-output-directory>
python scripts/render_translation_ab.py <output-directory>
```

The runner clears machine translations in copies of the caches, fixes the OCR input, records image and model digests, alternates the two modes page by page, and saves request timings. Use a new output directory so old results cannot be mistaken for fresh inference. The rendering check evaluates layouts and contour constraints; it does not replace visual review of bubble detection or damage to artwork.

Dialogue mode uses IDs such as `dN` and caches `full_translation`, `mapping_status`, and member results. A single source block can be mapped directly. Matching line counts produce `lines_unreviewed`; an uncertain split produces `whole_only`, leaving member translations empty. A whole translation is displayed only inside a trusted region whose members match exactly. The backend does not split sentences by character count. Block mode remains a fallback. Page text, sound effects, and punctuation receive candidate classifications rather than being silently discarded by short-text or Latin-letter heuristics.

The [structure and translation evaluation record](docs/structure-evaluation-2026-09-14.md) describes the fixed 62-page v5 run and its limitations. Automatic dialogue completion was 589/604 (97.5%) in dialogue mode and 543/604 (89.9%) in block mode. Block mode remains the default because automatic completion is not semantic accuracy. For later corrections to historical notes, see the [current status](进度汇总/当前进度_2026-09-14.md).

For a three-page smoke run, add `--limit 3` to the A/B command, then render the results. The historical three-page run completed 16/17 dialogue units in dialogue mode and 15/17 in block mode, with no pixels drawn outside approved contours. These are protocol and layout regression results only.

To create an independent structure annotation template:

```text
python scripts/annotate_structure.py <cached-page.json> <annotation.json> --init
```

Fill in `expected_dialogues`, `expected_panel_order`, or per-block `expected_block_attributes` (`vertical` / `text_kind`), then use `--score`. Scoring reports strict exact matches, order-independent best Jaccard coverage, and attribute accuracy separately. `--init` does not overwrite existing annotations unless `--force` is supplied. Unfilled templates do not produce accuracy scores, and coverage is not interchangeable with exact accuracy.

To aggregate annotations:

```text
python scripts/score_structure_annotations.py <annotation-directory> --output annotation-score.json
```

Unannotated pages are counted separately and excluded from averages. Unknown IDs, duplicate membership, and empty units are reported as `label_warnings` so invalid labels are not mistaken for model performance.

### Local model service

Both clients use `127.0.0.1:11439` with cloud features disabled. The default project model path is `.models\ollama` on Windows and `.models/ollama` on macOS. Model files are not tracked in Git. To start the service manually on Windows:

```powershell
$env:OLLAMA_HOST = '127.0.0.1:11439'
$env:OLLAMA_MODELS = (Resolve-Path '.models\ollama').Path
$env:OLLAMA_NO_CLOUD = '1'
$env:OLLAMA_MAX_LOADED_MODELS = '1'
$env:OLLAMA_NUM_PARALLEL = '1'
ollama serve
```

To inspect installed models and loaded-model GPU usage:

```powershell
$env:OLLAMA_HOST = '127.0.0.1:11439'
ollama list
ollama ps
```

The Windows default is the locally validated `qwen3:4b-q6k` (Q6_K, approximately 3.3 GB). The reader also offers `qwen3:4b` and `qwen3:8b`. The project's setup notes record no official `qwen3:9b` tag; 8B was used for comparison. Translation requires Ollama at the address above. The original `scripts/setup.py` downloads macOS binaries and cannot perform a complete Windows installation.

With network access available, download the OCR models using:

```powershell
.\.venv\Scripts\python.exe -X utf8 scripts\prepare_models.py
```

## Data, caches, and logs

- macOS page cache: `~/Library/Application Support/KomaReader/pages/`.
- Windows page cache: `%LOCALAPPDATA%\KomaReader\pages\`.
- Linux backend page cache: `$XDG_DATA_HOME/KomaReader/pages/`, or `~/.local/share/KomaReader/pages/` when unset.
- Cache filenames use the image content's SHA-256. Renaming an image preserves cache reuse; different images with the same filename do not share a cache.
- OCR log: `.runtime/ocr.log`; model download log: `.runtime/model-download.log`; Windows launcher log: `.runtime\windows-launch.log`.
- Virtual environments, `.runtime`, `.models`, build outputs, and Python caches are excluded from Git.

## Development and validation

macOS:

```sh
zsh scripts/build.sh
.venv/bin/python -m unittest discover -s tests -v
.venv/bin/python backend/worker.py --image samples/01.webp --operation both --status .runtime/test-status.json
```

Windows:

```powershell
.\.venv\Scripts\python.exe -X utf8 -m unittest discover -s tests -v
.\.venv\Scripts\python.exe -X utf8 backend\worker.py --image samples\01.webp --operation both --status .runtime\test-status.json
```

Implementation entry points:

- `Sources/KomaReader.swift`: native macOS window, folder selection, reading, overlays, and editing.
- `windows_app.py`: PySide6 reader, zooming, overlays, structure diagnostics, and single-instance control.
- `backend/worker.py`: single-page OCR, caching, translation, response validation, and error persistence.
- `backend/overlay.py`: text region analysis and Chinese overlay geometry.
- `backend/structure.py` and `backend/panels.py`: dialogue membership, reading order, and panel candidates.
- `scripts/build.sh`: builds and locally signs the macOS app using the system Swift compiler.
- `启动Windows.bat`: launches Windows Python and Ollama with startup logging.
- `tests/`: regressions for cache isolation, overlay constraints, missing/duplicate IDs, manual-edit preservation, structure, and evaluation integrity.

Each page runs in a separate Python process, releasing OCR memory when it finishes. Uncached pages reload the OCR models. Text detection runs on the CPU; Manga OCR uses MPS or CUDA when available. Translation starts with a 4B model and can use other installed models according to the machine's resources.

## Validation status and limitations

- The native macOS app has previously been built and locally signed; the Windows reader runs through its launcher.
- The base Windows environment previously passed `pip check`. Before the 2026-09-19 upload, all 93 Python regression tests passed in `.venv-cuda`.
- Two original WebP samples completed OCR and Qwen3 4B translation, covering horizontal and vertical text.
- In the validated RTX 5070 setup, Manga OCR logged `Using CUDA`, and Ollama reported GPU use. `samples/01.webp` produced five text boxes and five valid translations with both 4B and 8B models.
- Responses are checked against the requested IDs. Placeholder text, empty translations, duplicate IDs, and missing entries produce explicit errors. Existing caches and manual edits are preserved.
- A fixed 62-page manga set has undergone automatic A/B and layout evaluation. Independent labels and human semantic review are still insufficient for overall quality acceptance. Vertical text, handwriting, sound effects, low-resolution images, and reading order across panels can still fail; pages 16 and 24 have known structure defects.
- Chinese mode uses white-background overlays, not seamless inpainting or professional typesetting. Freehand OCR box editing, cross-page character memory, glossaries, translated-image export, ZIP/PDF input, and recursive subfolders are not supported yet.
- Local models can refuse, mistranslate, or return malformed output. The application retains source text and errors; a valid translation is not guaranteed for every request.

## Upstream projects and licenses

- [Mokuro](https://github.com/kha-white/mokuro): GPL-3.0; manga text detection and line-by-line OCR.
- [Manga OCR](https://github.com/kha-white/manga-ocr): Japanese text recognition.
- [Comic Text Detector](https://github.com/dmMaze/comic-text-detector): manga text detection.
- [Ollama](https://github.com/ollama/ollama): local model service.
- [Qwen3](https://github.com/QwenLM/Qwen3): translation model family.

Model weights and dependencies are not included in Git. Each component remains subject to its upstream license.

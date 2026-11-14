# Shared backend and evaluation

[Project overview](../README.en.md)

Run all commands from the repository root using the chosen platform environment. Shared components: `backend/`, `scripts/`, `tests/`, `samples/`. Windows UI tests are in `windows/tests/`.

## Core regressions

Keep checks for translation protocols and recovery, manual edits, panels/dialogues, cleanup and layout, cache boundaries, cancellation, and client behavior. Inputs are generated; private manga, historical manifests, and real model services are not required.

| Scope | Entry point |
| --- | --- |
| Shared backend | `tests/test_worker.py`, `test_translation_service.py`, `test_panels.py`, `test_structure.py`, `test_overlay.py` |
| macOS display and vision inputs | `macos/tests/test_*.py` |
| Model process protocol and array boundaries | `core/python/test_model_worker.py` |
| Rust protocol, translation, display, queue and cancellation | `tools/rust_migration/verify_*.py`, using current Python assertions and generated fixtures |
| Native cache and low-level behavior | Unit tests in `core/src/` |
| Swift configuration, panning, layout and transport | `macos/tests/*Tests.swift` |
| Windows Qt client and service selection | `windows/tests/` |

From the repository root, use a Python environment with the project dependencies installed:

```sh
python scripts/check_regressions.py
```

After preparing native dependencies, build and test the Rust core, then enable native and Swift checks:

```sh
python3 tools/rust_migration/cargo.py test -- --test-threads=1
python3 tools/rust_migration/cargo.py build
python scripts/check_regressions.py --native --swift
```

`--native` defaults to `core/target/debug/koma-core`; set `KOMA_TEST_CORE` to use another binary built from the current source. `--swift` requires macOS and Xcode command-line tools. Add `--windows` after installing PySide6. Requested checks fail on missing dependencies rather than reporting a skipped suite as a pass. The runner cleans its temporary `.runtime/` outputs on exit; individual verifiers accept a fresh `--output` directory to retain reports.

Commit test source, not local images, models, runtime caches, or coverage reports. `.gitignore` excludes generated outputs. Real model quality, GPU inference, and human review of the complete window experience remain outside these offline core regressions.

## Historical acceptance and evaluation

On 2026-11-14, historical model/A-B/annotation tools, 35-page migration comparisons, screenshot reviews, and corpus-specific scripts were removed. Their source remains in the [pre-cleanup revision](https://github.com/Liuyy026/screen-capture-translator-public/tree/75c7276a93f1d6a55ff09592cf1b2dbd3aede91f). Results remain in `docs/` and `进度汇总/`; historical commands require their corresponding source revision and are not current entry points or new validation evidence.

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

The Windows default is the locally validated `qwen3:4b-q6k` (Q6_K, approximately 3.3 GB). The reader also offers `qwen3:4b` and `qwen3:8b`. The project's setup notes record no official `qwen3:9b` tag; 8B was used for comparison. Translation requires Ollama at the address above. The original `macos/scripts/setup.py` downloads macOS binaries and cannot perform a complete Windows installation.

With network access available, download the OCR models using:

```powershell
.\windows\.venv\Scripts\python.exe -X utf8 scripts\prepare_models.py
```

Page caches are named by image-content SHA-256; originals remain read-only. Platform guides describe client log and cache locations. Shared evaluation scripts may still use the repository-root `.runtime/`. Linux backend caches use `$XDG_DATA_HOME/KomaReader/pages/` or `~/.local/share/KomaReader/pages/`. Each page runs in a separate Python process; detection uses CPU and Manga OCR uses available MPS/CUDA.

[Path migration and verification](layout-migration.md)

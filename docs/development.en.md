# Shared backend and evaluation

[Project overview](../README.en.md)

Run all commands from the repository root using a Python environment with the project dependencies installed. Directories named `test/` or `tests/` at any level are for local use. `.gitignore` excludes them from Git.

## Core regressions

A public checkout can run the model process protocol and array boundary tests in `core/python/`. They use generated inputs and run without a model server.

```sh
python scripts/check_regressions.py
```

The runner also runs any available local Python suites. Add `--local` to require both the shared backend suite in `tests/` and the basic macOS suite in `macos/tests/`.

```sh
python scripts/check_regressions.py --local
```

| Option | Required local tests and dependencies |
| --- | --- |
| `--native` | Python assertions from `tests/` and `macos/tests/` used by `tools/rust_migration/verify_*.py`, plus a built Rust core |
| `--swift` | `macos/tests/*Tests.swift`, macOS and Xcode command-line tools |
| `--windows` | Python tests in `windows/tests/` and PySide6 |

Once the local tests and native dependencies are ready, build and test the Rust core, then run the native and Swift checks.

```sh
python3 tools/rust_migration/cargo.py test -- --test-threads=1
python3 tools/rust_migration/cargo.py build
python scripts/check_regressions.py --native --swift
```

`--native` defaults to `core/target/debug/koma-core`. Set `KOMA_TEST_CORE` to use another binary built from the current source. A requested suite fails if its local tests or dependencies are missing. The runner cleans its temporary `.runtime/` outputs on exit. Individual verifiers accept a fresh `--output` directory to retain reports.

Keep local images, model weights, runtime caches and coverage reports out of Git. Real model quality, GPU inference and the full window experience require separate checks.

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

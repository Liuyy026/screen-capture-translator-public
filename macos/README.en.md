# Koma · macOS Rust candidate

[简体中文](README.md) | English | [Project home](../README.en.md)

This guide applies to `refactor/rust-core`, not the stable `main` application. Run all commands from the repository root. The candidate requires Apple Silicon, macOS 14+, Swift build tools (`xcrun swiftc`), Rust/Cargo, Python 3 and uv. It depends on the checkout, native OpenCV, a Python model environment and installed weights; it is not a standalone app package.

## First-time setup

Build OpenCV 4.11.0 inside the project. This downloads source/build tools and does not install system libraries:

```sh
python3 -m venv .runtime/build-tools
.runtime/build-tools/bin/python -m pip install cmake==3.31.6 ninja==1.11.1.4
python3 tools/rust_migration/build_opencv.py \
  --directory .runtime/native-deps \
  --cmake .runtime/build-tools/bin/cmake \
  --ninja .runtime/build-tools/bin/ninja
```

Reuse a working project environment and weights if already prepared; do not recreate them unnecessarily. For a new environment using an existing oMLX service:

```sh
uv venv --python 3.12 macos/.venv
uv pip install --python macos/.venv/bin/python -r macos/requirements.lock
macos/.venv/bin/python scripts/prepare_models.py
python3 macos/scripts/setup_reading_model.py
```

These commands download OCR/detection weights and the optional Anime LaMa restoration model (about 197 MiB). Configure an image-capable model in oMLX yourself for visual assistance. Runtime inference reads installed weights offline.

Alternatively, after preparing OpenCV, use `python3 macos/scripts/setup.py` to install Python dependencies, OCR, a project-local Ollama and Qwen3 4B, and build the candidate. That installer may start local Ollama to download the model. It does not install Rust or build OpenCV. Then run `python3 macos/scripts/setup_reading_model.py` for restoration weights. Existing oMLX users do not need the Ollama installer.

## Build and run

For daily use, double-click **[启动 Rust 候选版.command](../启动%20Rust%20候选版.command)** at the repository root or `macos/启动.command`. Both use `scripts/build_and_run.sh --open`: reuse the existing build and activate an already-running candidate. Missing app/core artifacts or a moved checkout trigger a build and startup check. If setup is incomplete, the launcher keeps the error visible and points to this guide; it does not automatically install dependencies or download models.

```sh
# Build, restart this checkout's candidate, check 10 seconds of survival:
./macos/scripts/build_and_run.sh --verify
# Build only:
zsh macos/scripts/build.sh
# Open an existing build:
open 'macos/dist/Koma Rust 候选版.app'
```

The bundle identifier is `local.koma.reader.rust`. Both double-click entry points target this candidate. Development mode `--verify` rebuilds and restarts it; daily `--open` does not. The candidate build does not replace the stable app. Rebuild after moving the checkout. `--logs` streams logs and `--debug` starts LLDB. Grant macOS folder access when requested. The survival check alone is not a full UI or translation test.

Open an image folder, configure the provider/endpoint/key/model, then process a page or folder. Use the original, text-box and Chinese modes to inspect results. Source and translation text are editable; save before retranslating a single entry. Manually saved translations are protected during batch processing. Drag the image to pan; zoom and normal scrolling remain available.

## Connections and isolated data

The candidate stores data under `~/Library/Application Support/KomaReaderRust/`:

| Item | Purpose |
| --- | --- |
| `config.json` | Provider, endpoint, selected model and optional key |
| `pages/` | Ordinary OCR/translation results |
| `pages/vision-v1/` | Independent visual-assistance results |
| `reading-native-v1/` | Native display assets |
| `native-core.log` | Native service diagnostics |

Stable `KomaReader` configuration/pages can be copied read-only into candidate files on first use. Subsequent edits stay independent. Ordinary and vision edits are also not automatically synchronized.

Use the settings UI to persist connections. Defaults are `http://127.0.0.1:8000/v1` for oMLX and `http://127.0.0.1:11439` for Ollama. Keys can be saved in a local plaintext file with permissions `0600`; disabling key persistence keeps them session-only. The candidate does not access Keychain. Invalid configuration is reported without silently overwriting the file. Quit the app before editing it manually.

Rust only manages local Ollama processes it starts at the designated endpoint with no API key. Existing services and oMLX are not taken over. Application shutdown cleans up owned inference processes; model reuse can retain memory until release or exit.

## Processing and layout

Swift talks to the persistent Rust service through JSONL pipes. Rust owns task queues, cancellation, OCR page assembly, structure analysis, translation HTTP, codecs, persistence, contours, cleanup and background compositing. Python only runs installed CTD/Manga OCR/LaMa inference with required preprocessing. Swift retains Chinese layout and drawing. The old `backend/` and `macos/backend/` workers remain references, not candidate business entry points.

Visual assistance sends the full page (JPEG, maximum long edge 1775 pixels), existing OCR and coordinates to an image-capable oMLX model in batches of up to eight blocks. Proposed source corrections require confirmation; no new OCR boxes are added automatically. Better translation quality is not guaranteed.

Chinese is laid out left-to-right and top-to-bottom, measuring glyph widths against each line's contour. Connected bubbles, irregular shapes and narration receive bounded layouts. Pixel masks limit cleanup and local restoration; broad white strips are no longer used. Installed LaMa supports complex backgrounds; native fallback restoration is weaker. Reconstructed texture may contain artifacts and is not exact recovery of the original drawing.

Display assets are separate from translation caches, so existing translations can be reformatted without translation requests. Incomplete or unsafe layouts retain the image and report the reason in the sidebar. Pure punctuation and unchanged numbers can remain as originally drawn. These rules do not correct OCR omissions or semantic mistakes.

## Validation

After preparing native dependencies, run basic tests without real model weights or translation services:

```sh
python3 tools/rust_migration/cargo.py test
macos/.venv/bin/python -m unittest discover -s core/python -p 'test_*.py'
```

Completed validation covers 35 real OCR pages, pixel-identical final rendering of 244 existing translations, simulated translation pipelines, controller and real-window flows, restart cache reuse and actual application startup. See the [root results](../README.en.md), [full report](../docs/rust-core-validation.md) and [core tools](../core/README.md).

Private images, translation caches and `.runtime/rust-migration/pages.json` are not in Git. The complete corpus suite requires a local manifest and baselines; cloning alone does not provide these fixtures. `verify_display.py`, `verify_rendered_pages.py` and `run_reader_flow.py` exercise the Rust candidate. Legacy `macos/tools/review_chapter.py` targets the Python reference and stable cache paths; it is not proof of candidate behavior. Test renderers do not constitute an in-app translated-image export feature.

The original [November 7 vision report](../docs/macos-vision-1209806-2026-11-07.md) describes the historical Python/Keychain/white-overlay implementation. Current branch behavior is documented here and in the Rust validation report. Windows migration, independent packaging and broader quality evaluation remain pending.

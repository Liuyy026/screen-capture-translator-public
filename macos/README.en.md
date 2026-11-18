# Koma · macOS Rust candidate

[简体中文](README.md) | English | [Project home](../README.en.md)

This guide covers the macOS reader built from `main`. Run all commands from the repository root. The source build requires Apple Silicon, macOS 14+, Swift build tools (`xcrun swiftc`), Rust/Cargo, Python 3 and uv. It runs with the checkout, native OpenCV, a Python model environment and installed weights.

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

The source build is named `Koma Rust 候选版.app` and uses the bundle identifier `local.koma.reader.rust`. Use `--verify` to rebuild and restart after updating the source or moving the checkout. `--logs` streams logs and `--debug` starts LLDB. Grant macOS folder access when prompted. The 10-second check verifies startup.

Open an image folder, configure the provider, endpoint, optional key and model, then process a page or folder. Use the bottom bar to switch between original or enhanced image, text boxes and Chinese rendering. The zoom menu can fit the whole page, and clicking the page number opens chapter thumbnails. The dialogue pane is collapsible. Unsaved drafts survive dialogue and page switches during the current session. Save edits before retranslating an entry. Batch processing protects saved manual translations.

Use **Processing settings** (`处理设置`) in the top toolbar to choose enhancement, translation mode, provider and model. **Enhance chapter** applies the current settings to the folder and uses MetalFX when pre-enhancement is off. Changing enhancement settings reuses OCR, translations and manual edits within the selected translation mode. See the [upscaling guide in Chinese](../docs/macos-super-resolution.md).

Open **Page diagnostics** (`本页诊断`) at the bottom to see the image's pixel dimensions and the upscaling, OCR, translation and layout status. Click a flagged dialogue to jump to its editor. The panel can also reveal the local log in Finder.

When a box looks like blank space or screentone, the reader pauses translation and rendering for that box. Check the source image and select **Confirm as text** (`确认为文字，恢复处理`) in the dialogue pane to resume. See the [Mac workflow audit (Chinese)](../docs/macos-flow-audit.md) for the save, cancellation and export regression checks.

### Keep the original bubble

Select a dialogue and choose **Keep original bubble** (`保留原框`) in the editor. All blocks grouped in the detected bubble are kept together. Later processing skips translation, cleanup and typesetting for that bubble. If the outline cannot be determined, the app preserves the selected OCR box. Choose **Translate this bubble** (`翻译此框`) to resume processing. The selection is saved locally for the current translation mode, and any existing translation remains available.

### Export manga

1. Process the pages to export in the current translation mode. Click **Export manga** (`导出漫画`) in the toolbar.
2. Choose the whole chapter or enter selected pages. Use the page numbers shown in the reader. You can mix single pages and ranges, such as `1, 3, 5–8`.
3. Set the folder name and destination, then start the export. A new installation defaults to your Documents folder. The app remembers the destination locally after a successful export.

The app composites existing Chinese layouts onto the enhanced images and saves local PNGs with the original chapter numbers, such as `001.png`, `003.png` and `005.png`. It uses the current enhancement settings, with MetalFX when pre-enhancement is off. Preserved bubbles and areas without usable translations retain the original text. If the folder name is taken, the app adds a number. After export, reveal the folder in Finder from the status bar.

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

Display assets are separate from translation caches, so existing translations can be reformatted without translation requests. Incomplete or unsafe layouts retain the image and report the reason in the bottom status area, with the full translation available in the dialogue pane. Pure punctuation and unchanged numbers can remain as originally drawn. These rules do not correct OCR omissions or semantic mistakes.

## Validation

After preparing native dependencies, run basic tests without real model weights or translation services:

```sh
python3 tools/rust_migration/cargo.py test
macos/.venv/bin/python -m unittest discover -s core/python -p 'test_*.py'
```

The November 8, 2026 migration validation covered 35 real OCR pages, pixel-identical rendering of 244 existing translations, simulated translation, controller and window flows, restart cache reuse and application startup. See the [root results](../README.en.md), [full report](../docs/rust-core-validation.md) and [core tools](../core/README.md).

Run current offline core regressions with `scripts/check_regressions.py --native --swift`; see the [development guide](../docs/development.en.md). Historical corpus, model and screenshot acceptance scripts have been removed; their results remain historical evidence.

The [November 7 vision report](../docs/macos-vision-1209806-2026-11-07.md) covers the older Python implementation with Keychain and white overlays.

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

These commands download OCR/detection weights and the optional Anime LaMa restoration model (about 197 MiB). For visual assistance, configure oMLX as described in the [visual assistance guide](#visual-assistance-omlx). Runtime inference reads installed weights offline.

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

Open an image folder, configure the provider, endpoint, optional key and model, then process a page or folder. Use the bottom bar to switch between original or enhanced image, text boxes and Chinese rendering. The zoom menu can fit the whole page, and clicking the page number opens chapter thumbnails. The dialogue pane is collapsible. Drag the divider to resize the dialogue pane. Expand **Typesetting** (`排版`) for the selected dialogue to change its font, size and direction. **Reflow page** (`重排本页`) uses saved translations. Unsaved drafts survive dialogue and page switches during the current session. Save edits before retranslating an entry. Batch processing protects saved manual translations.

Use **Processing settings** (`处理设置`) in the top toolbar to choose enhancement, translation mode, provider and model. **Enhance chapter** applies the current settings to the folder and uses MetalFX when pre-enhancement is off. Changing enhancement settings reuses OCR, translations and manual edits within the selected translation mode. See the [upscaling guide in Chinese](../docs/macos-super-resolution.md).

Open **Page diagnostics** (`本页诊断`) at the bottom to see the image's pixel dimensions and the upscaling, OCR, translation and layout status. Click a flagged dialogue to jump to its editor. The panel can also reveal the local log in Finder.

When a box looks like blank space or screentone, the reader pauses translation and rendering for that box. Check the source image and select **Confirm as text** (`确认为文字，恢复处理`) in the dialogue pane to resume. See the [Mac workflow audit (Chinese)](../docs/macos-flow-audit.md) for the save, cancellation and export regression checks.

### Correct missed text and move a translation

Click **圈选补识别** in the dialogue inspector and drag around one missed line or column on the original page. Check the Japanese, then choose a new dialogue or prepend or append it to an existing one. You can enter the full Chinese translation at the same time. Saving just the Japanese keeps the previous translation in the inspector for review or retranslation before typesetting.

In Chinese view, open the inspector and select a dialogue. Drag inside its frame to move the translation, or drag either side handle to change its width. Cleanup stays at the source text. The chapter saves the position for reading and PNG export. Click **恢复位置** under typography to return to automatic placement and keep the font size.

### Keep the original bubble

Select a dialogue and choose **Keep original bubble** (`保留原框`) in the editor. All blocks grouped in the detected bubble are kept together. Later processing skips translation, cleanup and typesetting for that bubble. If the outline cannot be determined, the app preserves the selected OCR box. Choose **Translate this bubble** (`翻译此框`) to resume processing. The selection is saved locally for the current translation mode, and any existing translation remains available.

### Export manga

1. Click **Export manga** (`导出漫画`) and choose the whole chapter or specific pages. Use the page numbers in the reader, for example `1, 3, 5–8`.
2. Choose ordinary or vision-assisted translations for this export. The default follows your reader setting. Review the selected pages for missing recognition, text that needs checking and differing manual translations in the other mode. You can change the export source, open a page to inspect it or continue with the selected source. Each mode keeps its own edits.
3. If only ordinary results exist, the vision source can inherit OCR and saved manual edits. To include other translations in the vision edition, translate the page in vision mode first. If only vision results exist, process the page in ordinary mode before exporting an ordinary edition.
4. Set the destination and folder name, then start the export. If the source changes before confirmation, review the updated results. A new installation defaults to Documents and remembers the destination after a successful export.

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

Koma keeps the translation model in memory for the next page. Switching models or providers asks the previous service to unload the model used in this session. Closing the last reader window or quitting Koma releases any remaining translation models used in the session. To free memory while reading, click **释放当前模型** (Release current model) in processing settings. The model stays selected and loads again the next time a page needs translation. The button can also release the selected model if an older version of Koma left it loaded.

oMLX and Ollama release memory for the requested model. If another client is using the same model on the same oMLX server, unloading it can interrupt that request. When Koma quits, it also stops its OCR worker and any local Ollama process it started at `127.0.0.1:11439`.

## Visual assistance (oMLX)

In **Processing Settings**, choose **Visual Assistance** as the translation mode. Koma sends the current page image, OCR text and text coordinates to your configured oMLX service. Pages 1 and 2 send only the current page image. Starting on page 3, Koma also sends the previous page's original image from the same chapter as a reference. Page numbers follow chapter order for single-page processing, whole-chapter processing and individual retranslation. Translations and proposed OCR corrections apply only to the current page.

Each batch handles up to eight text boxes, disables model reasoning, enforces a JSON schema for translations and OCR suggestions, and caps output at 8,192 tokens. Use an oMLX service and vision model that support multiple images, disabling reasoning and structured output. Text-only translation continues to use the existing workflow.

Koma sends JPEG images with a maximum long edge of 1775 pixels and uses existing OCR boxes. Review and save any suggested OCR corrections you want to apply.

## Processing and layout

Swift talks to the persistent Rust service through JSONL pipes. Rust owns task queues, cancellation, OCR page assembly, structure analysis, translation HTTP, codecs, persistence, contours, cleanup and background compositing. Python only runs installed CTD/Manga OCR/LaMa inference with required preprocessing. Swift retains Chinese layout and drawing. Differential checks read the historical Python implementation from a pinned Git revision. The reader builds and runs from the Mac sources in this repository.

Automatic layout normally uses horizontal Chinese text and measures glyph widths against each line's contour. It can choose vertical text for tall, narrow regions. The dialogue editor lets you set the direction explicitly. Connected bubbles, irregular shapes and narration receive bounded layouts. Pixel masks limit cleanup and local restoration; broad white strips are no longer used. Installed LaMa supports complex backgrounds; native fallback restoration is weaker. Reconstructed texture may contain artifacts and is not exact recovery of the original drawing.

Display assets are separate from translation caches, so existing translations can be reformatted without translation requests. Incomplete or unsafe layouts retain the image and report the reason in the bottom status area, with the full translation available in the dialogue pane. Pure punctuation and unchanged numbers can remain as originally drawn. These rules do not correct OCR omissions or semantic mistakes.

## Validation

In a public checkout, run the model process protocol tests without real model weights or translation services.

```sh
macos/.venv/bin/python scripts/check_regressions.py
```

After preparing native dependencies, run the Rust unit tests with `python3 tools/rust_migration/cargo.py test`.

The November 8, 2026 migration validation covered 35 real OCR pages, pixel-identical rendering of 244 existing translations, simulated translation, controller and window flows, restart cache reuse and application startup. See the [root results](../README.en.md), [full report](../docs/rust-core-validation.md) and [core tools](../core/README.md).

With the local test files in place, `macos/.venv/bin/python scripts/check_regressions.py --local --native --swift` runs the shared backend, macOS Python, native differential and Swift regression suites. See the [development guide](../docs/development.en.md) for the files each option requires. Historical corpus, model and screenshot acceptance scripts have been removed; their results remain historical evidence.

The [November 7 vision report](../docs/macos-vision-1209806-2026-11-07.md) covers the older Python implementation with Keychain and white overlays.

# Koma · macOS

The macOS reader now offers **Text translation / Vision assist** with oMLX. Vision assist sends the page image, OCR text and original coordinates to a vision-capable model; the current preferred local model is `Qwen3.8-27B-Uncensored-MLX-4bit`. OCR corrections are suggestions for manual review, never automatic source replacements. Images are bounded to a 1775-pixel longest edge. Results and edits are stored separately under `~/Library/Application Support/KomaReader/pages/vision-v1/`; text-mode results remain in `pages/`. Existing OCR and manual translations may seed the first visual result, but later edits do not synchronize between modes. Missed OCR regions and sound effects are not automatically added, and visual input does not guarantee better translations or absence of refusals. See the [local evaluation](../docs/macos-vision-1209806-2026-11-07.md).

[简体中文](README.md) | English | [Project overview](../README.en.md)

Keep the full repository. Requires Apple Silicon, macOS 14+, Swift command-line tools, and uv. Setup downloads OCR/Ollama/Qwen3 models and creates `macos/.venv`. Run these commands from the repository root:

```sh
python3 macos/scripts/setup.py
```

Build only (no dependency installation or model startup):

```sh
zsh macos/scripts/build.sh
```

## Running on macOS

The macOS application targets Apple Silicon and macOS 14 or later. After setup, double-click **启动.command** in this directory, or open **macos/dist/Koma 漫画翻译.app**.

1. Click **打开文件夹** (Open folder) and select a folder containing image files directly.
2. Click **识别并翻译** (Recognize and translate). The **更多** (More) menu also offers OCR-only processing and folder translation.
3. Switch between **原图 / 文字框 / 中文** (Original / Text boxes / Chinese).
4. Edit the text in the right pane and click **保存修改** (Save changes). Changing the Japanese source clears its previous translation.
5. Save any edits before clicking **重新翻译** (Translate again) under an individual line.

The two original WebP test pages in `samples/` at the repository root contain horizontal and vertical Japanese text. They verify the processing flow, not recognition accuracy on real manga.


## Development and data

Run from the repository root:

```sh
macos/.venv/bin/python -m unittest discover -s tests -v
macos/.venv/bin/python backend/worker.py --help
macos/.venv/bin/python backend/worker.py --image samples/01.webp --operation both --model qwen3:4b --status macos/.runtime/test-status.json
```

Reader: `macos/Sources/KomaReader.swift`. Dependencies: `macos/requirements.txt` and `macos/requirements.lock`. Do not mix them with Windows requirements.

Environment: `macos/.venv/`; Ollama executable and OCR logs: `macos/.runtime/`; app: `macos/dist/`. Models stay in the repository-root `.models/`. Ollama uses `127.0.0.1:11439` with cloud features disabled.

Page cache: `~/Library/Application Support/KomaReader/pages/`. The app records the repository location at build time; rebuild after moving the repository. The macOS OCR stack pins Manga OCR 0.1.16, Transformers 4.45.2, PyTorch 2.5.1, and SciPy 1.17.1; the installer downloads qwen3:4b. The State property wrapper is accessed through an alias to work with the macOS 27 Command Line Tools SDK without the SwiftUI macro plugin.

[Shared development](../docs/development.en.md) · [Path mapping](../docs/layout-migration.md)

For development, build and launch with `./macos/scripts/build_and_run.sh --verify` from the repository root. The Codex Run action uses the same script. Use `--logs` to inspect app logs or `--debug` for LLDB.

On first use, macOS may ask for Desktop access because this checkout and its environment live on the Desktop. Approve that system prompt to let the app start its Python worker; a pending prompt can leave processing waiting before OCR begins.

[Verified installation and sample translation report](../docs/macos-verification-2026-11-07.md) (Chinese, 2026-11-07).

## Translation services: Ollama / oMLX

Open the service settings in the reader, choose Ollama or oMLX, enter its endpoint and optional masked API key, fetch the model list, choose a model, and save. oMLX defaults to `http://127.0.0.1:8000/v1`; Ollama defaults to `http://127.0.0.1:11439`. The application connects to an existing oMLX service and leaves its lifecycle to oMLX.

During development, the provider, endpoint, model and optional API key are stored outside the repository in `~/Library/Application Support/KomaReader/config.json`. This is a plaintext file with mode `0600`, readable and writable by the current user, and saves use atomic replacement. Uncheck **将 API Key 保存在本地配置文件** (Save API key in local configuration) to keep the key for the current run only and remove it from that provider's persisted configuration. The app does not access or prompt for Keychain; Keychain integration is deferred until the app is stable.

When the file is absent, existing provider, endpoint and model preferences migrate without reading old Keychain secrets. Invalid JSON, unsupported versions and access errors identify the file to repair and block automatic connection and overwriting. Quit the app before editing the file. Repair it, or move it aside as a backup, then restart.

Example (`apiKey` is an empty string when authentication is unnecessary; each provider has at most one entry in `connections`):

```json
{
  "version": 1,
  "selectedProvider": "omlx",
  "connections": [
    {
      "provider": "omlx",
      "baseURL": "http://127.0.0.1:8000/v1",
      "model": "your-local-model",
      "apiKey": ""
    }
  ]
}
```

Switching services invalidates machine translations while preserving manual edits. See the [usage and real-sample verification report](../docs/omlx-integration-2026-11-07.md) (Chinese) for CLI flags, results and validation limits; its historical Keychain instructions are superseded by the local configuration described above.

Run isolated configuration checks without launching the app or any model:

```sh
mkdir -p macos/.build
xcrun swiftc -parse-as-library macos/Sources/TranslationLocalConfig.swift macos/tests/ConnectionConfigTests.swift -o macos/.build/connection-config-tests
macos/.build/connection-config-tests
```

## Dragging the image

Hold the left mouse button over the image and drag to pan. This works over both the original picture and translation regions, after zoom changes and after switching pages. A short click on a text region still selects its dialogue. Wheel and trackpad scrolling remain available.

Panning stops at the image boundary. When the entire image fits in the viewport, it stays in place.


## Chinese balloon layout

Chinese mode analyzes balloon contours and original text footprints without translation-service calls or changes to source images/page caches. It uses the installed Comic Text Detector's pixel masks to isolate lettering. Fragments in one balloon are typeset together; narrow-neck balloons may be separated. Rectangular captions and bottom-clipped balloons are supported. Chinese flows left-to-right using measured glyph widths and contour-aware line wrapping. Narration grouping respects panel dividers. Dark backgrounds use white text with a dark outline; light backgrounds use the inverse.

Install the optional local Anime LaMa restoration weights with `python3 macos/scripts/setup_reading_model.py` (about 197 MiB, checksum verified; [upstream source](https://github.com/Sanster/IOPaint/blob/main/iopaint/model/lama.py)). Reading never downloads models automatically. Without these weights, traditional local restoration remains available. Only selected ink pixels are composited back; reconstructed backgrounds are estimates and may retain artifacts.

Disposable masks and render assets are stored separately under `~/Library/Application Support/KomaReader/reading-v3/`. Image, translation, algorithm and model changes invalidate the derived assets without retranslating pages. Unchanged numbers are not counted as missing overlays. Suspected misrecognized footer numbers are preserved with a review notice; manual edits override this protection.

Cached white rectangles are not blindly used as fallback; image geometry and background are checked again. Incomplete translations or regions too small for legible text remain unchanged, with full translations and a notice in the inspector. Clicking a translated balloon selects its first dialogue fragment. This improves display only; it does not correct OCR or translation meaning, detect missing text, or guarantee coverage of complex backgrounds and unusual balloon shapes.

The read-only geometry helper is `macos/backend/reading_overlay.py`; tests require no running model:

```sh
macos/.venv/bin/python -m unittest discover -s macos/tests -p 'test_reading*.py' -v
xcrun swiftc macos/Sources/ReadingLayout.swift macos/tests/ReadingLayoutTests.swift -o macos/.build/reading-layout-tests
macos/.build/reading-layout-tests
```

`macos/.venv/bin/python macos/tools/review_chapter.py --folder /path/to/chapter --output macos/.runtime/chapter-review` renders existing translations with the native SwiftUI reader and checks glyph bounds, text completeness and unchanged inputs. Use `--pages 13,14,15` to sample pages. Visual review is still required; geometric checks cannot prove restoration quality or OCR completeness.

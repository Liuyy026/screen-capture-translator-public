# Koma · Local Manga Translation

[简体中文](README.md) | English

Native macOS and Windows readers with a shared local OCR/translation backend. The UI and translation target remain Chinese.

## Choose a platform

| Platform | Guide | Entry point |
| --- | --- | --- |
| macOS / SwiftUI | [macOS guide](macos/README.en.md) | `macos/安装.command`, `macos/启动.command` |
| Windows / PySide6 | [Windows guide](windows/README.en.md) | `windows/启动Windows.bat` |

```text
macos/      macOS: Sources, scripts, requirements
windows/    Windows: reader, launcher, scripts, requirements, tests
backend/    Shared OCR / translation / cache / structure / overlays
scripts/    Shared model preparation / annotation / evaluation
tests/      Shared backend tests and fixtures
samples/    Test images
docs/       Development and evaluation
进度汇总/   Historical plans and reports
```

Keep the full repository. Platform environments and client logs live in their respective directories. Models stay in `.models/` at the repository root.

[Shared development](docs/development.en.md) · [Path migration](docs/layout-migration.md) · [Historical progress](进度汇总/README.md)

[macOS installation and translation verification (2026-11-07, Chinese)](docs/macos-verification-2026-11-07.md): built and launched on Apple Silicon, with dependencies, models, and sample OCR/translation checked.

[Ollama / oMLX service settings and verification](docs/omlx-integration-2026-11-07.md): configurable endpoint, API key, and discovered model selection in both clients; macOS real-service sample tests completed.

## Features

- Supports WebP, PNG, JPG, JPEG, BMP, and TIFF, with natural filename sorting.
- Recognizes horizontal and vertical Japanese text and displays text boxes, source text, and Chinese overlays.
- Supports editing Japanese source text and Chinese translations; manually saved translations are preserved during batch translation.
- Supports translating an individual line again, folder batch processing, cancellation, and cache reuse.
- Keeps original images read-only. Recognition results, translations, errors, and overlay metadata are stored in a local cache.

Editing, single-line retranslation, and folder batch processing currently have UI controls in the macOS reader. The Windows validation build supports single-page processing, stopping a task, cache loading, and structure diagnostics. Its results pane is read-only; editing, retranslation, and folder queue controls are still planned.

## Validation status and limitations

The following validation results are historical records from the original project; this reorganization does not rerun real OCR, GPU, or model evaluation.

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

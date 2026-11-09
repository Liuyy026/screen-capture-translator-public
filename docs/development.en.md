# Shared backend and evaluation

[Project overview](../README.en.md)

Run all commands from the repository root using the chosen platform environment. Shared components: `backend/`, `scripts/`, `tests/`, `samples/`. Windows UI tests are in `windows/tests/`.

## Test files and GitHub scope

Test source in `tests/`, `macos/tests/`, `windows/tests/`, and `core/python/test_model_worker.py` is committed with the application source to check translation, layout, cache, and UI behavior. `tests/fixtures/` contains structure annotations with placeholder text; `samples/` contains images generated for this project.

Keep local manga originals, OCR/translation caches, models, test outputs, and coverage reports out of Git. Store evaluation inputs and outputs under the ignored `.runtime/` directory. `.gitignore` also excludes common test caches and reports. Application release packages should contain only runtime files; test source does not need to ship with the app.

Two shared regression tests require local fixed-set assets at these default paths and are reported as skipped when assets are unavailable:

- `.runtime/test-assets/images/00008.webp`: the original image matching the SHA-256 in `tests/fixtures/page8-panels.json`.
- `.runtime/test-assets/images/00012.webp` and `.runtime/test-assets/cache/e63072ae9b114c2ae42bab145bec866236632e8c61b1ddde6f31d897aff71bb1.json`: the corresponding original image and OCR cache.

Set `KOMA_TEST_IMAGE_DIR` and `KOMA_TEST_CACHE_DIR` to use existing asset directories without copying or uploading originals. Skipped local-asset tests do not count as validated behavior.

Three historical Real-CUGAN acceptance scripts tied to old asset directories have been removed from the current source. Their source snapshot and results remain available in the [Real-CUGAN handoff record](super-resolution-realcugan-handoff.md). Current integration checks remain in `tools/super_resolution/verify_integrated_helpers.py`, with cancellation checks in `tools/rust_migration/verify_sr_cancellation.py`; these do not reproduce every GPU check from the historical scripts.

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

The [structure and translation evaluation record](structure-evaluation-2026-09-14.md) describes the fixed 62-page v5 run and its limitations. Automatic dialogue completion was 589/604 (97.5%) in dialogue mode and 543/604 (89.9%) in block mode. Block mode remains the default because automatic completion is not semantic accuracy. For later corrections to historical notes, see the [current status](../进度汇总/当前进度_2026-09-14.md).

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

The Windows default is the locally validated `qwen3:4b-q6k` (Q6_K, approximately 3.3 GB). The reader also offers `qwen3:4b` and `qwen3:8b`. The project's setup notes record no official `qwen3:9b` tag; 8B was used for comparison. Translation requires Ollama at the address above. The original `macos/scripts/setup.py` downloads macOS binaries and cannot perform a complete Windows installation.

With network access available, download the OCR models using:

```powershell
.\windows\.venv\Scripts\python.exe -X utf8 scripts\prepare_models.py
```

Page caches are named by image-content SHA-256; originals remain read-only. Platform guides describe client log and cache locations. Shared evaluation scripts may still use the repository-root `.runtime/`. Linux backend caches use `$XDG_DATA_HOME/KomaReader/pages/` or `~/.local/share/KomaReader/pages/`. Each page runs in a separate Python process; detection uses CPU and Manga OCR uses available MPS/CUDA.

[Path migration and verification](layout-migration.md)

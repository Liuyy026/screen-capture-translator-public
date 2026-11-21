# Mac development and validation

Run commands from the repository root after installing the Python model environment and native OpenCV. [First-time setup](../macos/README.en.md)

## Regression checks

```sh
python3 tools/rust_migration/cargo.py test --locked -- --test-threads=1
python3 tools/rust_migration/cargo.py build --locked
macos/.venv/bin/python scripts/check_regressions.py --native
```

The default runner checks the model adapter and available local Python suites. The native suite compares the current Rust binary against pinned Python code and assertions extracted from Git into a temporary directory. It covers translation, visual processing, reading, cache and cancellation paths. A full clone works offline. For a shallow clone or source archive, point `KOMA_BASELINE_REPO` at a local full clone containing the pinned revisions. Building and running the application uses only the current Mac sources.

Use `--local` to require private Python suites in `tests/` and `macos/tests/`. Use `--swift` for local `macos/tests/*Tests.swift`. Keep private tests, images, credentials and weights outside Git. Set `KOMA_TEST_CORE` to test a specific binary. The default is `core/target/debug/koma-core`.

## Packaging

Start from a clean commit. Set `MACOSX_DEPLOYMENT_TARGET=14.0` before compiling release OpenCV. A build directory in `/private/tmp` avoids embedding a personal home path in native build metadata. Pass its directory containing the OpenCV source, build and installation to the packager.

```sh
python3 macos/release/package.py --version 0.22 --native-deps /path/to/native-deps --output dist/v0.22
(cd dist/v0.22 && shasum -a 256 Koma-0.22-macOS-arm64.zip > SHA256SUMS.txt)
```

The packager verifies architecture, minimum OS, library paths and ad hoc signatures, and records the source commit in the ZIP. Release data stays in `~/Library/Application Support/KomaReaderRelease/`. The source-built reader uses `KomaReaderRust/`.

## Historical acceptance and evaluation

On 2026-11-14, historical model/A-B/annotation tools, 35-page migration comparisons, screenshot reviews, and corpus-specific scripts were removed. Their source remains in the [pre-cleanup revision](https://github.com/Liuyy026/screen-capture-translator-public/tree/75c7276a93f1d6a55ff09592cf1b2dbd3aede91f). Results remain in `docs/` and `进度汇总/`; historical commands require their corresponding source revision and are not current entry points or new validation evidence.

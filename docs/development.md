# Mac 开发与验证

从仓库根目录运行下列命令。首次构建请先准备 Python 模型环境及原生 OpenCV。[安装与运行](../macos/README.md)

## 日常回归

```sh
python3 tools/rust_migration/cargo.py test --locked -- --test-threads=1
python3 tools/rust_migration/cargo.py build --locked
macos/.venv/bin/python scripts/check_regressions.py --native
```

默认入口运行模型协议测试，以及已经在本机准备好的 Python 用例。`--native` 运行当前 Rust 核心与历史实现的对照，包括翻译协议、视觉翻译、阅读显示、缓存和取消等。它在临时目录从 Git 历史提取固定版本的 Python 代码和断言，完整克隆可离线运行。浅克隆或源码包可将 `KOMA_BASELINE_REPO` 指向包含这些历史提交的本地完整克隆。应用构建及运行不需要这些历史文件。

`--local` 要求本机的 `tests/` 和 `macos/tests/` Python 套件，`--swift` 运行本机 `macos/tests/*Tests.swift`。这些本地用例、真实图片、模型和个人配置留在 Git 之外。`KOMA_TEST_CORE` 可以指定待测的核心文件，默认使用 `core/target/debug/koma-core`。

## 打包

使用干净的源码提交。OpenCV 的最低部署目标应为 macOS 14，在编译它之前设置 `MACOSX_DEPLOYMENT_TARGET=14.0`。为避免原生依赖的编译信息携带个人路径，发布用 OpenCV 的构建目录可设在 `/private/tmp`。然后传入含有 `build/`、`install/` 和原始 OpenCV 源码的目录。

```sh
python3 macos/release/package.py --version 0.22 --native-deps /path/to/native-deps --output dist/v0.22
(cd dist/v0.22 && shasum -a 256 Koma-0.22-macOS-arm64.zip > SHA256SUMS.txt)
```

打包器检查 CPU 架构、最低系统版本、动态库路径和临时签名，并将当前提交写入 ZIP 的 `版本.txt`。发行版使用 `~/Library/Application Support/KomaReaderRelease/`，开发版使用 `KomaReaderRust/`。

## 历史验收与评测

2026-11-14 精简了旧模型/A-B/标注评测工具、35 页迁移对照、截图验收以及依赖历史素材的专项脚本。相关历史源码可从 [精简前版本](https://github.com/Liuyy026/screen-capture-translator-public/tree/75c7276a93f1d6a55ff09592cf1b2dbd3aede91f) 恢复，结果记录仍保留在 `docs/` 和 `进度汇总/`；旧报告中的命令只适用于对应历史版本，不能视为当前测试入口或本轮验证结果。

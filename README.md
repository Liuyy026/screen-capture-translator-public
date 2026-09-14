# Koma · 本地漫画翻译（Mac 原型）

原生 SwiftUI 阅读器：打开包含 WebP 的文件夹，自动检测漫画文字，用 Manga OCR 识别日文，再通过本机 Ollama 翻译成简体中文。

## 启动

本机完成安装后，双击根目录的 **启动.command**，或打开 **dist/Koma 漫画翻译.app**。

1. 点击「打开文件夹」，选择**直接包含图片**的文件夹。支持 WebP、PNG、JPG、JPEG、BMP、TIFF，按文件名自然排序。
2. 点击右上角「识别并翻译」。也可以通过旁边的「更多」菜单只识别日文，或翻译整个文件夹。
3. 切换「原图 / 文字框 / 中文」。点击图上的框定位到右侧对白。
4. 右侧可以修改日文或中文，点击「保存修改」。修改日文会清除对应旧译文；保存的手动中文不会被批量翻译覆盖。
5. 重新翻译单句时点击该句下方「重新翻译」。操作前先保存编辑。
6. 批量处理可以停止，已经保存的页面和译文不会丢失。再次处理会复用缓存。

内置 `samples/` 是原创的非漫画内容测试图，包含横排和竖排日文，可用于检查流程。它不代表真实漫画上的识别准确率。

## 本地运行与数据

- 首次安装下载 Python 依赖、文字检测权重、OCR 权重和 Qwen3 4B 量化模型。
- 原图只读。翻译结果写入 `~/Library/Application Support/KomaReader/pages/`，按图片内容 SHA-256 缓存；文件重命名仍可命中，同名但内容不同的图片不会串页。
- 阅读位置和模型选择保存在 macOS 的 `local.koma.reader` 偏好设置中。
- Ollama 使用独立的 `127.0.0.1:11439` 端口和项目 `.models/ollama/`，关闭云端功能，只允许已下载的本地模型。
- OCR 日志在 `.runtime/ocr.log`，模型下载日志在 `.runtime/model-download.log`。
- 当前是开发原型，App 内记录了本项目绝对路径，**请保留整个项目目录，不要只把 App 单独移走**。移动项目后运行 `zsh scripts/build.sh` 重建。

## 实现

- `Sources/KomaReader.swift`：原生窗口、文件夹选择、WebP 阅读、文字框/中文覆盖、对白编辑、后台进程和取消。
- `backend/worker.py`：单页 OCR、缓存、按编号验证译文、分批翻译及错误保存。
- `scripts/build.sh`：使用系统 Swift 编译器生成并本地签名 `.app`，不需要完整 Xcode。
- `tests/test_worker.py`：验证缓存隔离、漏译/重复编号处理、手动编辑保留。

每页在独立 Python 进程中处理，内存会在该页完成后释放；代价是未缓存页面需要重复加载 OCR 模型。文字检测在 CPU 运行，Manga OCR 在本机已验证使用 MPS 加速。翻译模型采用 4B 起步，之后可对比其他型号。

## 当前限制

- 尚未用用户的真实漫画验证。竖排、手写、拟声词、低清图片及跨分镜阅读顺序可能出错。
- 中文模式是简单的白底覆盖，并非无痕擦字或专业汉化排版。长译文请在右侧阅读。
- 暂不支持补框/拆框、跨页人物记忆、术语表、导出汉化图片、ZIP/PDF 和子文件夹递归。
- 本地模型可能拒译、误译或返回不符合格式的内容；本地运行不保证任何特定内容都能翻译。程序保留原文和错误信息。
- 首次加载和每页速度取决于机器负载、页面文字量等；不承诺即时翻译。

## 开发与验证

```sh
zsh scripts/build.sh
.venv/bin/python -m unittest discover -s tests -v
.venv/bin/python backend/worker.py --image samples/01.webp --operation both --status .runtime/test-status.json
```

新环境安装见 `安装.command`。原型针对 Apple Silicon、macOS 14+，当前开发机为 M2 / 16GB。

### 本机验证记录（2026-09-11）

- 原生应用编译和本地签名通过。
- 两张自制 WebP 测试页通过实际 OCR → Qwen3 4B 翻译，包括横排和竖排。
- 测试页 01 的四处日文对白均识别成功；页眉英文也被识别，存在英文 OCR 错字。
- 在应用里点击「识别并翻译」完成测试页 02，得到「准备好了，走吧！」。
- 中文覆盖、翻页读取缓存、右侧修改译文并保存均已通过 UI 操作验证。
- 5 项后端测试通过；尚未针对真实漫画评估准确率或速度。

### 译文校验修复

- 输出结构按本次真实编号生成，重译编号 58 时不再以固定编号 1 作示例。
- 单句「重新翻译」只处理该句，其他失败的对白不会一起被重试。
- 每句独立校验；占位文字、空译文、重复编号和漏译明确标错，正确对应的其他译文保留。
- 阅读旧缓存时会识别已知占位文字并取消其中文覆盖；原始缓存和手动编辑保留。
- 10 项后端回归测试通过，涵盖以上情形。此修复处理输出格式和状态，不保证模型一定提供有效译文，也未重译用户报告的漫画内容。

## 上游项目

- [Mokuro](https://github.com/kha-white/mokuro)：GPL-3.0；提供漫画检测和逐行 OCR 流程。
- [Manga OCR](https://github.com/kha-white/manga-ocr)：日文文字识别。
- [Comic Text Detector](https://github.com/dmMaze/comic-text-detector)：漫画文字检测。
- [Ollama](https://github.com/ollama/ollama)：本地模型服务。
- [Qwen3](https://github.com/QwenLM/Qwen3)：当前翻译模型系列。

模型和依赖不纳入 Git；各组件遵循其上游许可证。

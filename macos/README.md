# Koma · macOS

简体中文 | [English](README.en.md) | [项目首页](../README.md)

请保留完整仓库。要求 Apple Silicon、macOS 14+、Swift 编译工具和 uv。双击本目录 `安装.command`，或在仓库根目录执行以下安装命令；安装会下载 OCR/Ollama/Qwen3 模型并建立 `macos/.venv`：

```sh
python3 macos/scripts/setup.py
```

只构建应用（不安装依赖、不启动模型）：

```sh
zsh macos/scripts/build.sh
```

## macOS 启动

项目针对 Apple Silicon、macOS 14+ 开发。完成安装后，双击本目录的 **启动.command**，或打开 **macos/dist/Koma 漫画翻译.app**。

1. 点击「打开文件夹」，选择直接包含图片的文件夹。
2. 点击「识别并翻译」。也可以通过「更多」菜单只识别日文，或翻译整个文件夹。
3. 切换「原图 / 文字框 / 中文」查看不同显示模式。
4. 在右侧编辑文本，点击「保存修改」；修改日文会清除对应旧译文。
5. 重新翻译单句前先保存编辑，再点击该句下方的「重新翻译」。

仓库根目录 `samples/` 中的两张原创 WebP 测试图包含横排和竖排日文，可用于检查流程，不代表真实漫画上的识别准确率。


## 开发与数据

以下命令在仓库根目录执行：

```sh
macos/.venv/bin/python -m unittest discover -s tests -v
macos/.venv/bin/python backend/worker.py --help
macos/.venv/bin/python backend/worker.py --image samples/01.webp --operation both --model qwen3:4b --status macos/.runtime/test-status.json
```

界面：`macos/Sources/KomaReader.swift`；依赖：`macos/requirements.txt`、`macos/requirements.lock`，不要与 Windows 混装。

环境：`macos/.venv/`；Ollama 可执行文件和 OCR 日志：`macos/.runtime/`；应用：`macos/dist/`。模型仍保存在仓库根目录 `.models/`。Ollama 使用 `127.0.0.1:11439`，关闭云端功能。

页面缓存：`~/Library/Application Support/KomaReader/pages/`。应用记录构建时的仓库位置；移动整个仓库后需要重新构建。macOS OCR 依赖已固定为 Manga OCR 0.1.16、Transformers 4.45.2、PyTorch 2.5.1、SciPy 1.17.1；安装器下载并默认使用 `qwen3:4b`。界面使用状态属性包装器别名，兼容缺少 SwiftUI 宏插件的 macOS 27 Command Line Tools SDK。

[共用开发说明](../docs/development.md) · [旧路径对照](../docs/layout-migration.md)

开发时可在仓库根目录运行 `./macos/scripts/build_and_run.sh --verify`，重新构建并验证启动；Codex 的 Run 按钮也使用同一入口。`--logs` 可查看日志，`--debug` 可进入 LLDB。

首次使用时，macOS 可能请求访问“桌面”文件夹，因为当前项目和 Python 环境放在桌面。请在系统弹窗中手动允许；等待授权期间，OCR 会停留在“准备处理”。

[本机安装与样例翻译验证报告（2026-11-07）](../docs/macos-verification-2026-11-07.md)。

## 翻译服务：Ollama / oMLX

在阅读器中打开接口/服务设置，选择服务，填写地址与可选 API Key，获取模型列表、选择模型后保存。oMLX 默认为 `http://127.0.0.1:8000/v1`，Ollama 默认为 `http://127.0.0.1:11439`。oMLX 需自行运行，应用不会启停它。

开发期间，服务、接口地址、模型和可选 API Key 保存在 `~/Library/Application Support/KomaReader/config.json`，不放入仓库。配置是明文文件，权限为 `0600`，仅当前用户可读写；每次保存均原子替换。设置中的「将 API Key 保存在本地配置文件」不勾选时，密钥只用于本次运行，并从该服务的持久配置中清除。应用不会读取、写入或弹出系统钥匙串授权；钥匙串接入留待稳定后再处理。

首次没有配置文件时，只迁移已有偏好中的服务、地址和模型，不读取旧钥匙串密钥。JSON 格式错误、版本不支持或权限异常会显示具体文件位置与修复提示，并阻止自动连接和覆盖原配置。手工修改前先退出应用；修复文件，或将它移走保留备份后，重新启动。

配置格式示例（不需要认证时 `apiKey` 填空字符串；`connections` 每个服务最多一项）：

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

切换服务后重新生成机器译文，手改译文继续保留。操作、CLI 参数和真实样例记录见[接入与验证说明](../docs/omlx-integration-2026-11-07.md)；该历史记录中的钥匙串说明已由上述本地配置方案替代。

无需启动应用或模型即可运行本地配置检查：

```sh
mkdir -p macos/.build
xcrun swiftc -parse-as-library macos/Sources/TranslationLocalConfig.swift macos/tests/ConnectionConfigTests.swift -o macos/.build/connection-config-tests
macos/.build/connection-config-tests
```

## 鼠标拖动图片

在图片上按住鼠标左键并拖动，即可平移查看超出视窗的部分；从原图或译文区域开始拖动都可以。放大、缩小或换页后仍可继续拖动。短按文字框仍用于定位对白，滚轮和触控板也可继续滚动。

拖动会限制在图片边界内；缩小到整张图片都能显示时，图片保持原位。

## 视觉辅助翻译（oMLX）

右侧「翻译方式」可选择「普通翻译」或「视觉辅助」。后者保留现有文字检测和 Manga OCR，将整页图片、文字坐标与日文一起交给 oMLX 视觉模型；需要服务中已有支持图片的模型。当前优先使用本机 `Qwen3.8-27B-Uncensored-MLX-4bit`，仍可通过模型菜单更换。

选择「视觉辅助」后，点击「识别并翻译」，也可只识别、重译单句或批量处理。原文与中文仍可手动编辑；图片中发现的疑似 OCR 错误显示为「画面复核建议」，点击填入日文后，核对并保存才会修改原文。中日共用词的相同译文会保留并提示核对。

视觉结果保存到 `~/Library/Application Support/KomaReader/pages/vision-v1/`，普通结果仍在 `pages/`。两种模式单独保存编辑；首次建立视觉结果时，可复用普通模式已有 OCR 和手动译文，后续修改不自动跨模式同步。图片长边最多 1775 像素；当前仍依赖 OCR 找框，漏检文字、拟声词不会自动补框。视觉模式不保证比普通翻译更准确，也不保证模型不会拒译。

实现仅位于 `macos/backend/vision_worker.py` 与 Mac 界面；不改变 Windows 和共享后端的处理方式。[1209806 实测记录](../docs/macos-vision-1209806-2026-11-07.md)。

```sh
macos/.venv/bin/python -m unittest discover -s macos/tests -p 'test_vision_worker.py' -v
macos/.venv/bin/python macos/backend/vision_worker.py --help
```


## 中文气泡排版

「中文」模式在打开已有结果时，单独分析封闭气泡的安全内部空间，不调用模型，也不改写漫画或页面缓存。同一气泡的碎片文字合并排版；有狭窄连接的双气泡会尝试分开。原图/文字框模式仍按原 OCR 框显示。

中文默认从左到右、从上到下横排。每行按气泡的实际轮廓寻找可用宽度，长句可以利用异形气泡的安全空间，不局限于单个内接矩形；不同译文保留段落分隔。字号以页面宽度统一计算，并保留可读下限。点击中文区域可定位其第一段对白，右侧仍可逐段编辑。

白底气泡只擦除文字笔画及邻近残留；灰底和网点背景使用局部背景修复，旁白按阅读顺序合并为横排段落，并加细描边以便阅读，不再铺长条纯白底。局部修复根据周围图像估计纹理，可能留下轻微痕迹，并非精确还原被原字遮住的内容。原图和译文缓存不被改写。

旧缓存中的白色矩形不会直接作为回退覆盖；打开页面时重新检查背景和文字范围。译文不完整、排版空间不足或背景不可靠时保留原图，右侧明确提示“已有译文暂未覆盖”，并显示完整译文。此项改善显示，不自动修正 OCR 或翻译语义，也不保证手写字、复杂背景、漏检文字和所有异形气泡都能覆盖。

显示分析入口为 `macos/backend/reading_overlay.py`，测试不需要启动模型：

```sh
macos/.venv/bin/python -m unittest discover -s macos/tests -p 'test_reading_overlay.py' -v
xcrun swiftc macos/Sources/ReadingLayout.swift macos/tests/ReadingLayoutTests.swift -o macos/.build/reading-layout-tests
macos/.build/reading-layout-tests
```

# 翻译服务设置与 oMLX 接入验证

日期：2026-11-07。macOS 原生界面与 Windows PySide6 界面均已加入服务设置；本次真实服务和图形界面实测在当前 Apple Silicon Mac 上完成。

## 在界面中使用

1. 打开应用右侧的“接口设置…”（Windows 为工具栏服务设置入口）。
2. 选择 **oMLX** 或 **Ollama**，填写地址与可选 API Key。
3. 点击“连接并获取模型”（Windows：“测试连接并获取模型”），从服务返回的列表中选择翻译模型，再保存。
4. 使用“识别并翻译”，或 macOS“更多”中的整文件夹翻译。

默认地址：

| 服务 | 默认接口地址 | 模型编号 |
| --- | --- | --- |
| oMLX | `http://127.0.0.1:8000/v1`，也接受省略 `/v1` 的根地址 | 服务 `/v1/models` 返回的 ID |
| Ollama | `http://127.0.0.1:11439` | 服务 `/api/tags` 返回的名称，例如 `qwen3:4b` |

Windows 连接 Mac 上的 oMLX 时，应填写 Mac 的可访问局域网地址，而非 Windows 自己的 `127.0.0.1`；oMLX 需自行开启相应网络监听及鉴权。应用不会修改它的监听设置。

接口地址和各服务的模型选择会保存。macOS 可勾选将 API Key 保存在系统钥匙串；不勾选则仅在本次运行保留，并移除该地址此前保存的 Key。Windows API Key 仅保留在当前运行的内存中，重开后需重新填写。密钥不放入地址、命令行参数、译文缓存或日志。更换地址时需重新确认认证信息。

如果重新编译后 macOS 请求桌面或钥匙串权限，请由用户在系统弹窗中授权；应用不代替用户更改隐私权限。

## 服务与缓存行为

- oMLX 由外部应用管理。连接、刷新模型列表不会发起模型加载请求；正常翻译请求可由 oMLX 自行触发加载。应用不会启动、停止或手动卸载 oMLX 模型。
- macOS 仅对默认本机 Ollama 地址提供自动启动；自定义 Ollama 地址由外部服务管理。切换服务或退出时，只清理应用自己启动的 Ollama。
- Windows 启动器读取保存的服务设置；选择 oMLX 或自定义 Ollama 地址时，不再额外启动本机 Ollama。设置值在 Python 内比较，不展开到批处理命令中。
- 页面处理开始时固定服务、地址、密钥及模型；处理中禁用服务和模型修改。延迟的旧连接结果不能覆盖新设置。
- 机器译文的缓存身份包含服务、规范化地址和模型。切换服务后会重新生成机器译文，手动保存的中文继续保留。macOS 编辑和单句重译会保留结构、覆盖及服务来源等未知元数据。
- 请求使用对应服务的 JSON Schema 输出格式，保留空译文、重复编号、漏译和截断校验；不会静默改走另一后端。

## 命令行

以下命令在仓库根目录执行。需要鉴权时，由调用环境提供 `KOMA_TRANSLATION_API_KEY`；不要把实际密钥加入参数或网址。

```sh
macos/.venv/bin/python backend/worker.py --list-models --provider omlx --base-url http://127.0.0.1:8000/v1
macos/.venv/bin/python backend/worker.py --image samples/02.webp --operation both --provider omlx --base-url http://127.0.0.1:8000/v1 --model gemma-4-26B-A4B-it-MLX-4bit --status macos/.runtime/status.json
macos/.venv/bin/python backend/worker.py --image samples/02.webp --operation both --provider ollama --base-url http://127.0.0.1:11439 --model qwen3:4b --status macos/.runtime/status.json
```

oMLX 示例模型是本次机器上已有的模型；其他机器应先列出其实际模型 ID。未填写 `--provider` 时保留原 Ollama 默认行为。Windows 可使用 `windows/.venv/Scripts/python.exe` 或 CUDA 环境运行同一后端。

## 本次真实验证

- 通过当前 oMLX 的认证接口获取 **7 个模型**；界面显示 API Key 缺失/不正确时的认证失败提示，正确配置后可列出并选择模型。
- 原生界面选用 `gemma-4-26B-A4B-it-MLX-4bit`，两张 WebP 样例共 **6 个文字块**全部返回结果，错误字段为空；其中含横排、竖排日文及一个英文页眉。中文模式和右侧日中对照均检查。
- 从界面切回 Ollama，选用 `qwen3:4b`，两张样例重新翻译成功；再切回 oMLX，Gemma 重新翻译成功。两次缓存快照分别记录正确的 provider/base_url，未把旧后端结果冒充新推理。
- 第二张样例无损另存为带独立元数据的 PNG，确认其缓存不存在后，以 oMLX 执行完整 OCR → 翻译 → 缓存写入，用时约 6.03 秒，输出“准备好了。走吧！”。这是翻译模型已加载时的一张简单样例，不是模型冷启动基准。
- 在当前服务仍是 oMLX 时测试本机 Ollama，然后取消设置：确认仅测试启动的 Ollama 被停止，主窗口保持打开，当前连接仍为 oMLX。
- 退出并用普通 `macos/启动.command` 重开后，恢复 oMLX 地址、Gemma 模型和钥匙串中的认证，界面显示“本地模型就绪”；没有自动启动多余的 Ollama。
- 测试前后 oMLX 服务进程保持同一个 PID，本次没有修改它的配置或重启该服务，也没有下载新的 oMLX 模型。原始样例图片保持不变。

本次 oMLX 实际输出：

| 图片 | OCR 原文 | Gemma 实际译文 |
| --- | --- | --- |
| samples/01.webp | 今日はいい天気だね | 今天天气真好啊 |
| samples/01.webp | ＫＯＭＡ，ＯＯＲＴＥＳＴ．０１ | KOMA, OORTEST.01 |
| samples/01.webp | そうだね。 | 是啊。 |
| samples/01.webp | 一緒に散歩しよう | 一起去散步吧 |
| samples/01.webp | ちょっと待って！ | 等一下！ |
| samples/02.webp | 準備できたよ。行こう！ | 准备好了。走吧！ |

英文页眉的 `OCR TEST` 被原有日文 OCR 误识别为 `OORTEST`；该错误仍存在。样例验证证明调用和显示流程可用，不代表真实漫画的普遍识别或翻译准确率。中文覆盖沿用现有实现。

## 自动检查与边界

- 共用 Python 后端：运行 **75 项**，**73 项通过、2 项因历史素材未提供而跳过**；包含新增的 11 项协议、认证、重定向和跨服务缓存测试。
- Windows 服务配置及路径专项：**22 项通过**。在这台 Mac 的离屏 Qt 环境中运行全部 51 项：49 项通过，2 项是此前已复现的 Windows 字体注册/像素断言失败；尚未进行 Windows 实机启动器、跨机或 CUDA 验证。
- 真实编译的 Swift 缓存编辑测试通过，覆盖编辑/重译保留元数据、未命中 ID 和损坏 JSON 不覆盖原文件。
- macOS 应用构建、本地签名、脚本语法和 Git 空白检查通过。API Key 未写入本任务源码或运行记录。

证据位于 `macos/.runtime/omlx-integration/`，已被 Git 忽略：

- `omlx-final-sample-results.json`、`ollama-sample-results.json`：两个后端的实际缓存快照。
- `fresh-omlx-pipeline.json`、`omlx-requests.jsonl`：无页面缓存的完整流程及真实请求计时。
- `final-ready.png`、`settings-omlx.png`：原生界面与接口设置截图；设置截图中的密钥为遮蔽显示。
- `backend-tests.log`、`windows-services-tests.log`、`windows-all-tests.log`：测试记录。
- `build-ui.log`：最终构建与启动检查。

源码和说明保留为本地可审阅修改，未提交或推送。

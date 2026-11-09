# Real-CUGAN macOS helper 交接

完成日期：2026-11-08（Asia/Shanghai）。独立副本基线 d24a9325699d8b9102c354b46025ed42dce22996。

> 2026-11-09 维护说明：本文记录当时的验收过程。三个 Real-CUGAN 阶段性脚本已从当前主分支移除，源码保留在 [52ca8ae 的 macOS 测试目录](https://github.com/Liuyy026/screen-capture-translator-public/tree/52ca8ae15c94f89a2e118db392fde9b303aac8a1/macos/tests)。下方旧命令仅用于历史追溯，需使用当时源码及素材；结果不代表当前版本已重新实测。当前检查入口见 [开发说明](development.md)。

## 交付范围与整合入口

已实现并实测 `macos/.build/koma-sr-realcugan`。Swift/Core Image 负责格式及 EXIF 方向归一、调用原生 arm64 ncnn/Vulkan 神经网络推理、Lanczos 精确收尾到调用方指定尺寸。运行不依赖 Python、PyTorch、venv 或服务。没有修改主项目、共享模型链接、稳定应用、UI、Rust 共用管线、主构建脚本或 README。未启动或停止 oMLX/Ollama。

源码：

- `macos/tools/RealCUGANUpscaler.swift`
- `macos/scripts/build_realcugan.sh`
- `macos/scripts/prepare_realcugan.sh`、`realcugan_manifest.py`（Python 仅用于准备阶段的来源清单）
- 历史验收脚本：`realcugan_review.py`、`realcugan_protocol_review.py`、`realcugan_cancel_review.py`（已移除，源码见页首固定版本链接）
- `docs/licenses/RealCUGAN-model-MIT.txt`

整合者 cherry-pick 本交付提交即可取得源码。**还必须复制独立运行资产**（Git 不含权重和引擎）：

```
/Users/lyy-studio/Desktop/project111/本地ai翻译/超分候选实验/20261108-front-sr/realcugan/.runtime/super-resolution/realcugan/realcugan-ncnn-vulkan-20220728-macos/
/Users/lyy-studio/Desktop/project111/本地ai翻译/超分候选实验/20261108-front-sr/realcugan/.runtime/super-resolution/realcugan/manifest.json
/Users/lyy-studio/Desktop/project111/本地ai翻译/超分候选实验/20261108-front-sr/realcugan/.runtime/super-resolution/realcugan/LICENSE-model-MIT.txt
```

目标 checkout 保持相同相对目录后重新运行 `macos/scripts/build_realcugan.sh`。也可重新运行准备脚本下载，zip SHA256 已钉住。默认从 helper 的 `macos/.build/` 向上定位 checkout；应用打包时可用 `KOMA_REALCUGAN_ROOT` 指定包含 engine 和 models-se 的发行目录，不能指到上一级资产根目录。

## 协议与语义

```
macos/.build/koma-sr-realcugan --capabilities
macos/.build/koma-sr-realcugan --input /absolute/input.webp --output /absolute/job/output.png --width 1558 --height 2160 --preset conservative
```

成功 stdout 恰好一个 JSON 对象，含 engine、width、height、elapsed_ms、preset、scale、backend_version、engine_sha256、model_sha256。所有引擎日志送 stderr；非零退出表示失败。路径允许空格及中文。输入及已有输出绝不覆盖。输出必须是调用方安排的不存在的 PNG 文件；父目录可创建。

| preset | 上游参数 | 实际模型 |
| --- | --- | --- |
| conservative | `-n -1` | `models-se/up{2,3,4}x-conservative` |
| standard | `-n 0` | `models-se/up{2,3,4}x-no-denoise` |
| denoise | `-n 3` | `models-se/up{2,3,4}x-denoise3x` |

取足够覆盖目标宽、高的最小 2/3/4 倍模型，随后精确缩小。没有用普通插值代替神经网络；不支持的 preset 明确失败。最大倍率 4x，输出每边不超过 16384。已达标或缩小的调用明确失败，调用方须先跳过。长边/高度计算及保持比例由调用方负责，helper 严格执行指定尺寸；整数取整可能让某一边略超过 2 倍而选择 3 倍，这是按足够目标的规则工作。

能力清单的 available 检查引擎可执行及全部九组模型存在，不代表每台机器 GPU 实际成功；GPU 初始化/推理失败会非零退出，绝不 CPU 或其他引擎静默回退。固定 GPU 0、tile 128、线程 `1:1:1`。未做硬件自适应调优。

缓存身份至少包含 backend_version、engine_sha256、model_sha256、preset、目标尺寸及有效图身份。`model_sha256` 为按源码 required 固定顺序，对九组 bin/param 内容 SHA256 字符串串接后再 SHA256；`model_manifest_sha256` 是这一值的兼容别名，**并非 manifest.json 文件哈希**。模型内容变化会改变能力身份，不仅依赖文件时间。完整逐文件 hash 另见 manifest.json。

本模块不拥有 OCR/翻译缓存，**整合端仍须实现和验收**：原图与有效图身份分离、增强后 OCR/清字/显示坐标一致、旧设置任务取消及结果拒收、原图手改缓存保留、关闭增强基线一致。不能把此 helper 实测当作整条 Rust 管线验收。

## 取消与隐私

Swift 使用 posix_spawn，不设置新进程组，因此 ncnn 与调用方创建的 helper job group 相同。macOS Foundation Process 实际会新建子进程组，已通过实测发现并移除。TERM/INT 转发给唯一的 ncnn 子进程并 waitpid 回收；spawn PID 发布期间屏蔽信号，子进程恢复正常信号掩码。上游程序没有再派生子进程。

中间目录 `.koma-realcugan-UUID` 位于 **output.parent**，不使用系统临时目录。成功、失败、正常取消均 atexit 清理。SIGKILL 无法执行清理；Rust 必须杀整个 job group，然后删除其拥有的 job 目录（包括隐藏目录、可能的部分 output）。仅 SIGKILL helper PID 不足以保证子进程退出，这属于所有外部子进程的强杀边界。

所有素材及推理均本地处理，未上传漫画。真实页仅从指定 pages.json 读取，原始文件和解码像素 hash 前后相同。

## 来源、身份与许可

- 上游发行包：https://github.com/nihui/realcugan-ncnn-vulkan/releases/tag/20220728
- zip：https://github.com/nihui/realcugan-ncnn-vulkan/releases/download/20220728/realcugan-ncnn-vulkan-20220728-macos.zip
- zip SHA256：`0df908cbb98b480f85897221b96d37b0bdb70f82d81b2c7037fe950dd5c0fa33`
- universal x86_64/arm64 引擎 SHA256：`a59aa9acd89115e33d7d71d7e413b405237833f331bdc87d4e20099af0e5e819`
- 使用 models-se，模型组合 SHA256：`0040507c2304648b86e2849c446ccb2e5ae552397fb23c69fddbc0b848775b4d`
- backend_version：`koma-realcugan-1`
- 模型来源：https://github.com/bilibili/ailab/tree/main/Real-CUGAN
- 引擎和模型上游分别是 MIT 许可，保留发行包 LICENSE 和仓库内模型 MIT 文本。上游 ncnn/MoltenVK 依赖许可仍应在对外打包时按完整上游依赖要求保留；本任务是本地候选测试，没有发布安装包。

`otool -L` 确认发行可执行程序仅动态依赖系统 frameworks/lib，不需额外共享 dylib 或 venv。程序通过 ncnn Vulkan/MoltenVK 使用 Apple GPU，stderr 实际报告 `Apple M5 Max`，没有指定 CPU。

## 测试环境与命令

实际系统报告：Mac Studio / Mac17,14，Apple M5 Max，18 CPU 核，40 GPU 核，64 GB RAM，macOS 27.0.1 (26A434)，Swift 6.4。硬件仅按本机报告记录。性能测试使用 fcntl.flock 独占上一级 `gpu-test.lock`；不能排除其他系统 GPU 负载。

历史验收时在独立副本根目录执行并通过（需使用当时源码和本地素材）：

```sh
macos/scripts/prepare_realcugan.sh
macos/scripts/build_realcugan.sh
macos/.build/koma-sr-realcugan --capabilities
macos/.venv/bin/python macos/tests/realcugan_protocol_review.py
macos/.venv/bin/python macos/tests/realcugan_review.py --manifest /Users/lyy-studio/Desktop/project111/本地ai翻译/screen-capture-translator-rust/.runtime/rust-migration/pages.json --output "$PWD/.runtime/super-resolution/realcugan/review" --lock /Users/lyy-studio/Desktop/project111/本地ai翻译/超分候选实验/20261108-front-sr/gpu-test.lock
macos/.venv/bin/python macos/tests/realcugan_cancel_review.py
KOMA_REALCUGAN_ROOT=/nonexistent macos/.build/koma-sr-realcugan --capabilities
bash -n macos/scripts/build_realcugan.sh macos/scripts/prepare_realcugan.sh
git diff --check
```

venv 为已有只读用途链接，仅运行 Pillow/numpy QA，没有安装或修改它。测试脚本不是应用运行依赖。protocol/cancel 脚本的默认 manifest 与 lock 指向本次约定目录，移植其他机器可调整其测试配置。

## 实际结果与耗时

真实页 00002/00005，均 1280×1775 WebP。**540/720/1080 是人工降采样长边**，用于配对观察；不能当作真实低清扫描的结论。18 次完整推理全部成功，输出按比例精确到指定整数尺寸，三档 stdout JSON/模型倍率正确。范围 519.8–1332.1 ms（进程总墙钟）。

| 页 | 输入长边→输出长边 | conservative ms | standard ms | denoise ms |
| --- | --- | ---: | ---: | ---: |
| 2 | 540→1440 | 558.7 | 573.4 | 573.9 |
| 2 | 720→2160 | 948.8 | 1055.8 | 1075.7 |
| 2 | 1080→2160 | 874.3 | 896.7 | 849.3 |
| 5 | 540→1440 | 572.9 | 519.8 | 532.0 |
| 5 | 720→2160 | 908.8 | 990.5 | 1027.3 |
| 5 | 1080→2160 | 891.2 | 914.1 | 1332.1 |

每次启动都是新 helper、新引擎进程，模型加载、GPU 初始化、输入归一、推理与输出落盘计入总墙钟；JSON elapsed_ms 从 helper 启动后的计时点开始，包含内容 hash 与模型初始化。最终矩阵首例 558.7 ms。开发中已先运行过其他测试，**不是重启机器后首个冷缓存数值**，没有假称首次安装/冷启动基准。

额外真实原始 WebP→1558×2160，standard 2x 模型：总墙钟 1763.4 ms，helper JSON 1743.3 ms。540 输入精确 4x denoise 成功，865.1 ms。带 EXIF orientation=6 的 JPEG 转正后尺寸和红蓝角位置通过像素检查。

取消：直接 TERM 4.3 ms 回收子进程；独立取消套件 INT 5.8 ms / 整组 TERM 7.3 ms / 整组 SIGKILL 5.9 ms，子进程均已回收，未生成输出。正常取消无中间文件；SIGKILL 中间目录残留被模拟调用方删除。父子 PGID 一致有实际记录。无资源能力清单 available=false、presets=[]、reason 非空。未知档位、零尺寸、覆盖输入、覆盖已有输出均非零退出，素材 hash 未变。

## 视觉检查与限制

已实际查看四张本地比较图：page2 540→1440、page5 720→2160、page2 1080→2160、原始 page2→2160。

- 540/720 人工低清：相较 Lanczos，气泡文字和线稿边缘更干净，部分字形更易分辨；但建筑窗线和砖缝被重画，失去的细节不能可靠恢复。
- 网点是明确损失：小网点/细纹变成平灰或条纹；denoise 最明显。1080 样本保守档保留更多纹理，标准档也会产生细纹重组和摩尔纹，不能声称“保守档无损”。
- 原始 WebP→2160 标准档：文字更锐，建筑高频纹理出现粗化、气泡背后网点出现斜向纹理；原始已清楚时增强未必更好，1440 阈值应跳过此原图。
- 只抽查了两页中的有限裁剪，未做全文 OCR 对照、人工字符准确率、完整章节验收、真实低清扫描或长时间稳定性基准。PSNR 仅记录人工配对像素误差，受网点重建和参考插值影响，不能代替可读性/语义/OCR结论。
- 发行引擎来自 2022 年，虽然在当前 GPU 实测通过，不代表未来系统或所有 Mac 兼容。颜色/透明素材没有专门验收；本次是真实黑白漫画及合成方向测试。

## 本地产物（不提交 Git）

全部结果目录：

```
/Users/lyy-studio/Desktop/project111/本地ai翻译/超分候选实验/20261108-front-sr/realcugan/.runtime/super-resolution/realcugan/review/
```

`results.json`：18 次推理结果、模型身份、耗时、文件/像素 hash、取消和负例；`protocol-results.json`：原生输入/4x/方向/进程组；`cancel-results.json`：三种取消与残留回收；`unavailable.json`：资源缺失；每次推理 `.stderr` 保留实际 GPU 日志。`compare-page{2,5}-{540,720,1080}-{1440,2160}.jpg` 与 `compare-native-page2-2160.jpg` 为可视裁剪，完整 PNG 也在同目录。`review.log` 在资产根目录。漫画、输出、权重及工具二进制均不纳入提交。

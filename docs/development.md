# 共用后端与评估

[项目首页](../README.md)

以下命令均在仓库根目录运行，使用所选平台的 Python 环境。`backend/`、`scripts/`、`tests/`、`samples/` 为共用内容；Windows 界面测试单独放在 `windows/tests/`。

## 测试文件与 GitHub 提交范围

`tests/`、`macos/tests/`、`windows/tests/` 和 `core/python/test_model_worker.py` 中的测试代码随源码提交，用于检查翻译、排版、缓存和界面行为。`tests/fixtures/` 保留结构标注和测试占位文字；`samples/` 是项目生成的示例图。

本地漫画原图、OCR/译文缓存、模型、测试输出和覆盖率报告不提交。评测输入与输出应放在已忽略的 `.runtime/` 中；`.gitignore` 也排除了常见测试缓存和报告。应用发行包只应包含运行所需文件，测试代码无需随应用分发。

两项依赖本地固定样本的共用回归测试默认读取以下位置，缺少素材时会显示为跳过：

- `.runtime/test-assets/images/00008.webp`：与 `tests/fixtures/page8-panels.json` 中 SHA-256 一致的原图。
- `.runtime/test-assets/images/00012.webp` 及 `.runtime/test-assets/cache/e63072ae9b114c2ae42bab145bec866236632e8c61b1ddde6f31d897aff71bb1.json`：对应原图及 OCR 缓存。

可通过 `KOMA_TEST_IMAGE_DIR`、`KOMA_TEST_CACHE_DIR` 指定已有素材目录，无需复制或上传原图。macOS 的 Real-CUGAN 协议复核读取当前仓库的 `.runtime/rust-migration/pages.json`，并依赖先前在 `.runtime/super-resolution/realcugan/review/` 生成的复核素材。跳过本地素材测试不代表这些项目已验证。

## Ollama 与模型

### 对白单元与 A/B 验证

后端缓存会保存 `dialogues`、成员块编号、阅读顺序和 `diagnostics`。翻译时可选择按文字块或完整对白单元组织请求：

```powershell
python backend\worker.py --image page.webp --operation translate --translation-unit dialogues --status status.json
```

固定测试集可分别用 `--translation-unit blocks` 与 `--translation-unit dialogues` 运行，再用 `scripts\compare_translation_units.py` 比较块级和对白级成功率。已有缓存可用 `scripts\migrate_structure.py` 补齐结构字段。

结构评估可用 `python scripts/evaluate_structure.py <缓存目录> --output structure-report.json`。该命令只在内存中迁移旧结构，保留原始缓存，报告每页 OCR 指纹、成员归属、阅读顺序连续性和需复核候选。连续性是结构完整性检查，不等同于人工阅读顺序正确率。

完整 A/B 可使用 `python scripts/run_translation_ab.py <原始缓存目录> <图片目录> <新的输出目录>`。脚本清空副本中的机器译文，固定 OCR、记录图片和模型摘要，逐页交替运行 blocks/dialogues，并保存每次请求耗时。输出目录必须是新目录，防止历史缓存冒充新推理。之后运行 `python scripts/render_translation_ab.py <输出目录>` 检查两组全部页面的布局和轮廓约束。此检查不能代替人工判断气泡是否误检或画面是否被误伤。

对白模式以 `dN` 为翻译目标，缓存 `full_translation`、`mapping_status` 及每个成员的结果。只有一个源块时可直接映射；多行一一对应时标为 `lines_unreviewed`；无法确认拆分时标为 `whole_only`，成员译文留空，整句仅在成员完全匹配的可信区域显示。暂不对无明确拆分的整句按字数硬切，逐块模式保留为回退。页面文字、拟声词和标点只标候选类型，不因短语或拉丁字母启发式直接丢弃。

固定 62 页的 v5 结果与局限见 [结构与翻译评估记录](structure-evaluation-2026-09-14.md)。v5 A/B 中对白模式自动完整率为 589/604（97.5%），逐块模式为 543/604（89.9%）；默认仍为逐块模式，因为自动完整率不等于语义准确率。

分镜 v5 的快速回归可用 `python scripts/run_translation_ab.py <原始缓存目录> <图片目录> <新的输出目录> --limit 3`，再运行 `python scripts/render_translation_ab.py <输出目录>`。历史 3 页回归中对白单元自动完成率为 16/17，逐块为 15/17；两组均无轮廓外绘制像素。该数字只用于协议和排版回归。

人工结构标注可先运行 `python scripts/annotate_structure.py <缓存页.json> <标注.json> --init`，在模板中填写 `expected_dialogues`、`expected_panel_order` 或按块填写 `expected_block_attributes`（`vertical`/`text_kind`），再用 `--score` 计算严格精确匹配率、顺序无关的最佳 Jaccard 覆盖率和属性字段准确率。已有标注文件默认不会被 `--init` 覆盖，确需重建时显式加 `--force`。未填写的模板不会生成虚假的准确率；两类指标分别保留，不能互相替代。

批量汇总使用 `python scripts/score_structure_annotations.py <标注目录> --output annotation-score.json`。未完成标注的页面会单独计数，不参与平均值；未知块编号、重复归属和空单元会作为 `label_warnings` 报告，避免标签错误被误当成模型准确率。


## 本地模型服务

macOS 和 Windows 都使用本机 `127.0.0.1:11439`，并关闭云端功能。模型文件放在项目 `.models\ollama`（macOS 为 `.models/ollama`），不纳入 Git。Windows 手动启动服务时：

```powershell
$env:OLLAMA_HOST = '127.0.0.1:11439'
$env:OLLAMA_MODELS = (Resolve-Path '.models\ollama').Path
$env:OLLAMA_NO_CLOUD = '1'
$env:OLLAMA_MAX_LOADED_MODELS = '1'
$env:OLLAMA_NUM_PARALLEL = '1'
ollama serve
```

检查已安装模型和 GPU 占用：

```powershell
$env:OLLAMA_HOST = '127.0.0.1:11439'
ollama list
ollama ps
```

当前 Windows 默认使用已验证的 `qwen3:4b-q6k`（Q6_K，约 3.3 GB）；`qwen3:4b` 和 `qwen3:8b` 仍可在界面中切换。官方没有 `qwen3:9b` 标签。完整翻译流程需要 Ollama 保持在上述端口运行。原 `macos/scripts/setup.py` 下载的是 macOS 二进制，不能用于 Windows 完整安装。

网络可用时下载 OCR 模型：

```powershell
.\windows\.venv\Scripts\python.exe -X utf8 scripts\prepare_models.py
```

页面缓存按图片内容 SHA-256 命名，原图保持只读。客户端日志与系统缓存位置见各平台说明。共用评估脚本仍可使用根目录 `.runtime/`。Linux 后端缓存使用 `$XDG_DATA_HOME/KomaReader/pages/`，未设置时为 `~/.local/share/KomaReader/pages/`。每页在独立 Python 进程中处理；检测在 CPU 运行，Manga OCR 使用可用的 MPS/CUDA。

[路径迁移与本次验证](layout-migration.md)

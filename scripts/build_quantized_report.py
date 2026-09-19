"""Build the comparison document for the 2026-09-14 quantized benchmark."""
from __future__ import annotations

import csv
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
BENCH = ROOT / ".runtime" / "benchmarks"
OUT = BENCH / "quantized-20260914"

RUNS = [
    ("qwen3:4b", "4B Q4_K_M（上轮稳定基线）", BENCH / "manga-final" / "qwen3_4b-stable-report.json", BENCH / "manga-final" / "model-caches" / "qwen3_4b"),
    ("qwen3:4b-q6k", "4B Q6_K", OUT / "q6-4b-run2" / "report.json", OUT / "q6-4b-run2" / "model-caches" / "qwen3_4b-q6k"),
    ("qwen3:4b-q8", "4B Q8_0", OUT / "q8-4b" / "report.json", OUT / "q8-4b" / "model-caches" / "qwen3_4b-q8"),
    ("qwen3:8b", "8B Q4_K_M（上轮稳定基线）", BENCH / "manga-final" / "qwen3_8b-stable-report.json", BENCH / "manga-final" / "model-caches" / "qwen3_8b"),
    ("qwen3:8b-q5k", "8B Q5_K_M", OUT / "q5-8b" / "report.json", OUT / "q5-8b" / "model-caches" / "qwen3_8b-q5k"),
]


def read_summary(path: Path):
    data = json.loads(path.read_text(encoding="utf-8"))
    return (data.get("models") or [data])[0]


def natural(name: str):
    import re
    return [int(x) if x.isdigit() else x for x in re.split(r"(\d+)", name)]


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    summaries = []
    for model, label, report, _ in RUNS:
        summary = read_summary(report)
        summaries.append({"model": model, "label": label, **summary, "report": str(report)})

    cache_maps = {}
    for model, _, _, cache_dir in RUNS:
        cache_maps[model] = {}
        for path in cache_dir.glob("*.json"):
            page = json.loads(path.read_text(encoding="utf-8"))
            cache_maps[model][path.stem] = {str(block.get("id")): block for block in page.get("blocks", [])}

    image_stems = sorted(cache_maps["qwen3:4b"], key=natural)
    columns = [item[0] for item in RUNS]
    csv_path = OUT / "translation-comparison.csv"
    with csv_path.open("w", encoding="utf-8-sig", newline="") as handle:
        fields = ["image", "id", "source"] + [f"{model}_translation" for model in columns] + [f"{model}_error" for model in columns]
        writer = csv.DictWriter(handle, fieldnames=fields)
        writer.writeheader()
        for image in image_stems:
            ids = sorted({block_id for model in columns for block_id in cache_maps[model].get(image, {})}, key=natural)
            for block_id in ids:
                blocks = [cache_maps[model].get(image, {}).get(block_id, {}) for model in columns]
                source = next((block.get("source", "") for block in blocks if block.get("source")), "")
                if not any((block.get("translation"), block.get("error")) for block in blocks):
                    continue
                row = {"image": image + ".webp", "id": block_id, "source": source}
                for model, block in zip(columns, blocks):
                    row[f"{model}_translation"] = block.get("translation", "")
                    row[f"{model}_error"] = block.get("error", "")
                writer.writerow(row)

    result = {
        "dataset": {"image_dir": r"C:\Users\18910\Downloads\1471793", "pages": 62, "eligible_blocks": 627},
        "runs": summaries,
        "artifacts": {"comparison_csv": str(csv_path), "run_root": str(OUT)},
        "quantization": {
            "4b_q6": "Qwen3-4B-Q6_K.gguf, direct local GGUF import",
            "4b_q8": "Qwen3-4B-Q8_0.gguf, direct local GGUF import",
            "8b_q5": "Qwen3-8B-Q5_K_M.gguf, direct local GGUF import",
        },
    }
    (OUT / "comparison.json").write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")

    lines = [
        "# 量化模型性能验证（2026-09-14）",
        "",
        "本轮沿用上轮 `C:\\Users\\18910\\Downloads\\1471793` 的 62 页、627 个可翻译块，并复用上轮 OCR 缓存；上一轮结果保留在 `.runtime/benchmarks/manga-final`。本轮结果集中放在当前目录。",
        "",
        "| 模型 | 页数 | 有效译文/块 | 有效率 | 平均页秒 | P95 页秒 | 输出 tok/s | 峰值显存 MB |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for item in summaries:
        lines.append(f"| {item['label']} | {item['pages_completed']}/{item['pages_expected']} | {item['valid_blocks']}/{item['eligible_blocks']} | {item['effective_translation_rate']:.2%} | {item['mean_page_seconds']:.2f} | {item['p95_page_seconds']:.2f} | {item['output_tokens_per_second']:.2f} | {item['peak_gpu_memory_mb']:.0f} |")
    lines += [
        "",
        "## 量化尝试说明",
        "",
        "- Ollama 0.34.0 的 `ollama create --quantize` 只接受 F32、F16、Q4_K_S、Q4_K_M、Q8_0；直接对已量化 Q4 模型请求 Q6/Q5 会被拒绝。",
        "- 为完成目标格式验证，本轮使用 Qwen 官方 GGUF 文件直接登记到本地 Ollama manifest：4B Q6_K、4B Q8_0、8B Q5_K_M，并用 `ollama show` 核对量化类型。",
        "- Q6/Q8/Q5 均完成 62 页翻译运行；每页 worker 成功，质量指标只统计非空、非原文复读且无错误的块。",
        "",
        "逐块译文和错误字段见 `translation-comparison.csv`；每个模型的原始日志、页面缓存、GPU 采样和 `report.json` 在对应子目录。",
    ]
    (OUT / "comparison.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()

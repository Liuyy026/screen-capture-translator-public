"""Benchmark the local manga translation pipeline with qwen3:4b and qwen3:8b.

The benchmark first creates an isolated OCR cache, then runs translation for
each model against that same cache. Results are written under .runtime so the
run is reproducible without changing the application's normal cache.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import time
from datetime import datetime, timezone


ROOT = Path(__file__).resolve().parents[1]
IMAGE_EXTENSIONS = {".webp", ".png", ".jpg", ".jpeg", ".bmp", ".tif", ".tiff"}


def natural_key(path: Path):
    import re

    return [int(part) if part.isdigit() else part.lower()
            for part in re.split(r"(\d+)", path.name)]


def cache_path(image: Path, local_appdata: Path) -> Path:
    digest = hashlib.sha256(image.read_bytes()).hexdigest()
    return local_appdata / "KomaReader" / "pages" / f"{digest}.json"


def gpu_sample():
    command = [
        "nvidia-smi",
        "--query-gpu=memory.used,memory.total,utilization.gpu,power.draw",
        "--format=csv,noheader,nounits",
    ]
    try:
        completed = subprocess.run(command, capture_output=True, text=True,
                                   encoding="utf-8", errors="replace", timeout=2)
        if completed.returncode != 0 or not completed.stdout.strip():
            return None
        values = [value.strip() for value in completed.stdout.splitlines()[0].split(",")]
        if len(values) < 4:
            return None
        return {
            "memory_used_mb": float(values[0]),
            "memory_total_mb": float(values[1]),
            "utilization_gpu_percent": float(values[2]),
            "power_draw_w": float(values[3]),
        }
    except (OSError, subprocess.SubprocessError, ValueError):
        return None


def run_worker(image: Path, operation: str, model: str, local_appdata: Path,
               run_dir: Path, metrics_path: Path | None = None, batch_size: int = 8,
               translation_unit: str = "blocks"):
    safe_name = image.stem
    log_dir = run_dir / "logs" / operation
    log_dir.mkdir(parents=True, exist_ok=True)
    status = run_dir / "status" / f"{operation}-{safe_name}.json"
    status.parent.mkdir(parents=True, exist_ok=True)
    stdout_path = log_dir / f"{safe_name}.stdout.log"
    stderr_path = log_dir / f"{safe_name}.stderr.log"
    environment = os.environ.copy()
    environment["LOCALAPPDATA"] = str(local_appdata)
    environment["OLLAMA_HOST"] = "127.0.0.1:11439"
    environment["OLLAMA_NO_CLOUD"] = "1"
    if metrics_path is not None:
        environment["KOMA_BENCHMARK_METRICS"] = str(metrics_path)
    else:
        environment.pop("KOMA_BENCHMARK_METRICS", None)
    command = [
        sys.executable, "-X", "utf8", str(ROOT / "backend" / "worker.py"),
        "--image", str(image), "--operation", operation,
        "--model", model, "--status", str(status),
        "--batch-size", str(batch_size),
        "--translation-unit", translation_unit,
    ]
    started = time.perf_counter()
    samples = []
    process = subprocess.Popen(command, cwd=ROOT, env=environment,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                text=True, encoding="utf-8", errors="replace")
    timed_out = False
    while process.poll() is None:
        sample = gpu_sample()
        if sample is not None:
            samples.append(sample)
        if time.perf_counter() - started > 900:
            timed_out = True
            process.kill()
            break
        time.sleep(0.5)
    stdout, stderr = process.communicate()
    elapsed = time.perf_counter() - started
    stdout_path.write_text(stdout, encoding="utf-8")
    stderr_path.write_text(stderr, encoding="utf-8")
    result = None
    for line in reversed(stdout.splitlines()):
        try:
            result = json.loads(line)
            break
        except json.JSONDecodeError:
            continue
    cache = cache_path(image, local_appdata)
    return {
        "image": image.name,
        "operation": operation,
        "model": model,
        "batch_size": batch_size,
        "translation_unit": translation_unit,
        "elapsed_seconds": elapsed,
        "returncode": process.returncode,
        "timed_out": timed_out,
        "ok": bool(result and result.get("ok")) and not timed_out,
        "worker_result": result,
        "cache": str(cache),
        "gpu_samples": samples,
        "stdout_log": str(stdout_path),
        "stderr_log": str(stderr_path),
    }


def page_quality(cache: Path):
    try:
        page = json.loads(cache.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        return {"cache_ok": False, "cache_error": str(exc), "blocks": []}
    blocks = page.get("blocks", [])
    dialogues = page.get("dialogues", [])
    eligible = [block for block in blocks
                if not block.get("edited") and block.get("overlay") != "skip"
                and block.get("text_kind", "dialogue") != "noise"]
    valid = []
    errors = []
    for block in eligible:
        translation = block.get("translation")
        error = block.get("error") or ""
        if isinstance(translation, str) and translation.strip() and not error:
            source_compact = "".join(str(block.get("source", "")).split())
            target_compact = "".join(translation.split())
            if source_compact and source_compact == target_compact:
                errors.append("原文复读")
            else:
                valid.append(block)
        else:
            errors.append(error or "空译文")
    dialogue_valid = [d for d in dialogues if str(d.get("translation", "")).strip()]
    return {
        "cache_ok": True,
        "model_in_cache": page.get("model", ""),
        "total_blocks": len(blocks),
        "eligible_blocks": len(eligible),
        "skipped_blocks": sum(1 for block in blocks if block.get("overlay") == "skip"),
        "valid_blocks": len(valid),
        "error_blocks": len(errors),
        "dialogue_count": len(dialogues),
        "translated_dialogues": len(dialogue_valid),
        "dialogue_translation_rate": len(dialogue_valid) / len(dialogues) if dialogues else None,
        "errors": errors,
        "page": page,
    }


def percentile(values, fraction):
    if not values:
        return None
    ordered = sorted(values)
    if len(ordered) == 1:
        return ordered[0]
    index = (len(ordered) - 1) * fraction
    low, high = math.floor(index), math.ceil(index)
    if low == high:
        return ordered[low]
    return ordered[low] + (ordered[high] - ordered[low]) * (index - low)


def summarize(model, records, metrics):
    successful = [record for record in records if record["ok"]]
    elapsed = [record["elapsed_seconds"] for record in successful]
    quality = [record.get("quality", {}) for record in records]
    eligible = sum(item.get("eligible_blocks", 0) for item in quality)
    valid = sum(item.get("valid_blocks", 0) for item in quality)
    error_count = sum(item.get("error_blocks", 0) for item in quality)
    dialogue_count = sum(item.get("dialogue_count", 0) for item in quality)
    translated_dialogues = sum(item.get("translated_dialogues", 0) for item in quality)
    responses = [item for item in metrics if item.get("response_received")]
    prompt_tokens = sum(item.get("prompt_eval_count") or 0 for item in responses)
    output_tokens = sum(item.get("eval_count") or 0 for item in responses)
    prompt_ns = sum(item.get("prompt_eval_duration") or 0 for item in responses)
    output_ns = sum(item.get("eval_duration") or 0 for item in responses)
    all_gpu = [sample for record in records for sample in record.get("gpu_samples", [])]
    peak_memory = max((sample["memory_used_mb"] for sample in all_gpu), default=None)
    peak_utilization = max((sample["utilization_gpu_percent"] for sample in all_gpu), default=None)
    total_elapsed = sum(elapsed)
    return {
        "model": model,
        "pages_expected": len(records),
        "pages_completed": len(successful),
        "pages_failed": len(records) - len(successful),
        "eligible_blocks": eligible,
        "valid_blocks": valid,
        "error_blocks": error_count,
        "effective_translation_rate": valid / eligible if eligible else None,
        "error_rate": error_count / eligible if eligible else None,
        "dialogue_count": dialogue_count,
        "translated_dialogues": translated_dialogues,
        "dialogue_translation_rate": translated_dialogues / dialogue_count if dialogue_count else None,
        "total_page_worker_seconds": total_elapsed,
        "mean_page_seconds": statistics.mean(elapsed) if elapsed else None,
        "p50_page_seconds": percentile(elapsed, 0.50),
        "p95_page_seconds": percentile(elapsed, 0.95),
        "max_page_seconds": max(elapsed) if elapsed else None,
        "first_page_seconds": elapsed[0] if elapsed else None,
        "mean_after_first_page_seconds": statistics.mean(elapsed[1:]) if len(elapsed) > 1 else None,
        "request_count": len(responses),
        "prompt_eval_tokens": prompt_tokens,
        "output_eval_tokens": output_tokens,
        "prompt_tokens_per_second": prompt_tokens / (prompt_ns / 1e9) if prompt_ns else None,
        "output_tokens_per_second": output_tokens / (output_ns / 1e9) if output_ns else None,
        "peak_gpu_memory_mb": peak_memory,
        "peak_gpu_utilization_percent": peak_utilization,
    }


def read_metrics(path: Path):
    rows = []
    if not path.exists():
        return rows
    for line in path.read_text(encoding="utf-8").splitlines():
        try:
            item = json.loads(line)
        except json.JSONDecodeError:
            continue
        if "response_received" not in item:
            item["response_received"] = "prompt_eval_count" in item
        rows.append(item)
    return rows


def copy_model_caches(images, local_appdata, destination):
    destination.mkdir(parents=True, exist_ok=True)
    for image in images:
        source = cache_path(image, local_appdata)
        if source.exists():
            shutil.copy2(source, destination / f"{image.stem}.json")


def build_differences(images, run_dir):
    left_dir = run_dir / "model-caches" / "qwen3_4b"
    right_dir = run_dir / "model-caches" / "qwen3_8b"
    rows = []
    for image in images:
        left_path = left_dir / f"{image.stem}.json"
        right_path = right_dir / f"{image.stem}.json"
        if not left_path.exists() or not right_path.exists():
            continue
        left = json.loads(left_path.read_text(encoding="utf-8"))
        right = json.loads(right_path.read_text(encoding="utf-8"))
        left_blocks = {str(block.get("id")): block for block in left.get("blocks", [])}
        right_blocks = {str(block.get("id")): block for block in right.get("blocks", [])}
        for block_id in sorted(set(left_blocks) | set(right_blocks), key=lambda value: int(value) if value.isdigit() else value):
            a, b = left_blocks.get(block_id, {}), right_blocks.get(block_id, {})
            if (a.get("translation"), a.get("error")) == (b.get("translation"), b.get("error")):
                continue
            rows.append({
                "image": image.name,
                "id": block_id,
                "source": a.get("source", b.get("source", "")),
                "qwen3_4b": a.get("translation", ""),
                "qwen3_8b": b.get("translation", ""),
                "qwen3_4b_error": a.get("error", ""),
                "qwen3_8b_error": b.get("error", ""),
            })
    (run_dir / "translation-differences.json").write_text(
        json.dumps(rows, ensure_ascii=False, indent=2), encoding="utf-8")
    with (run_dir / "translation-differences.csv").open("w", encoding="utf-8-sig", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(rows[0]) if rows else
                                ["image", "id", "source", "qwen3_4b", "qwen3_8b", "qwen3_4b_error", "qwen3_8b_error"])
        writer.writeheader()
        writer.writerows(rows)
    return rows


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--image-dir", required=True)
    parser.add_argument("--output-dir")
    parser.add_argument("--ocr-source-dir", help="reuse OCR cache and records from a prior run")
    parser.add_argument("--models", nargs="+", default=["qwen3:4b", "qwen3:8b"])
    parser.add_argument("--batch-size", type=int, default=8,
                        help="translation batch size; run separate output dirs with 1 and 8 for A/B")
    parser.add_argument("--translation-unit", choices=["blocks", "dialogues"], default="blocks")
    args = parser.parse_args()
    image_dir = Path(args.image_dir).resolve()
    images = sorted([path for path in image_dir.iterdir()
                     if path.is_file() and path.suffix.lower() in IMAGE_EXTENSIONS], key=natural_key)
    if not images:
        raise SystemExit(f"No supported images found in {image_dir}")
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run_dir = Path(args.output_dir).resolve() if args.output_dir else ROOT / ".runtime" / "benchmarks" / f"manga-{stamp}"
    run_dir.mkdir(parents=True, exist_ok=True)
    local_appdata = run_dir / "local-appdata"
    manifest = {
        "started_utc": datetime.now(timezone.utc).isoformat(),
        "image_dir": str(image_dir),
        "image_count": len(images),
        "models": args.models,
        "batch_size": max(1, args.batch_size),
        "python": sys.executable,
        "ollama_host": "127.0.0.1:11439",
        "ocr_cache": str(local_appdata),
    }
    (run_dir / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")

    if args.ocr_source_dir:
        source_dir = Path(args.ocr_source_dir).resolve()
        source_pages = source_dir / "local-appdata" / "KomaReader" / "pages"
        source_records = source_dir / "ocr-pages.json"
        if not source_pages.is_dir() or not source_records.exists():
            raise SystemExit(f"OCR source is incomplete: {source_dir}")
        target_pages = local_appdata / "KomaReader" / "pages"
        shutil.copytree(source_pages, target_pages, dirs_exist_ok=True)
        ocr_records = json.loads(source_records.read_text(encoding="utf-8"))
        print(f"OCR cache reused from {source_dir}: {len(ocr_records)} pages", flush=True)
    else:
        print(f"OCR prepass: {len(images)} pages")
        ocr_records = []
        for index, image in enumerate(images, 1):
            record = run_worker(image, "ocr", "qwen3:4b", local_appdata, run_dir)
            ocr_records.append(record)
            print(f"OCR {index:02d}/{len(images)} {image.name}: {record['elapsed_seconds']:.1f}s {'OK' if record['ok'] else 'FAIL'}", flush=True)
    (run_dir / "ocr-pages.json").write_text(json.dumps(ocr_records, ensure_ascii=False, indent=2), encoding="utf-8")
    if not all(record["ok"] for record in ocr_records):
        print("Warning: some OCR pages failed; translation totals cover cached pages only.", file=sys.stderr)

    model_summaries = []
    model_records = {}
    for model in args.models:
        safe_model = model.replace(":", "_").replace("/", "_")
        metrics_path = run_dir / f"{safe_model}-ollama.jsonl"
        records = []
        print(f"Translation benchmark: {model}")
        for index, image in enumerate(images, 1):
            record = run_worker(image, "translate", model, local_appdata, run_dir, metrics_path,
                                batch_size=max(1, args.batch_size), translation_unit=args.translation_unit)
            quality = page_quality(Path(record["cache"]))
            record["quality"] = {key: value for key, value in quality.items() if key != "page"}
            records.append(record)
            status = "OK" if record["ok"] and quality.get("cache_ok") else "FAIL"
            print(f"{model} {index:02d}/{len(images)} {image.name}: {record['elapsed_seconds']:.1f}s {status} valid={quality.get('valid_blocks', 0)}/{quality.get('eligible_blocks', 0)}", flush=True)
        metrics = read_metrics(metrics_path)
        summary = summarize(model, records, metrics)
        summary["metrics_file"] = str(metrics_path)
        model_summaries.append(summary)
        model_records[model] = records
        (run_dir / f"{safe_model}-pages.json").write_text(json.dumps(records, ensure_ascii=False, indent=2), encoding="utf-8")
        copy_model_caches(images, local_appdata, run_dir / "model-caches" / safe_model)

    differences = build_differences(images, run_dir)
    report = {
        "manifest": manifest,
        "ocr": {
            "pages": len(ocr_records),
            "completed": sum(1 for record in ocr_records if record["ok"]),
            "total_seconds": sum(record["elapsed_seconds"] for record in ocr_records),
        },
        "models": model_summaries,
        "translation_difference_count": len(differences),
        "artifacts": {
            "run_dir": str(run_dir),
            "differences_json": str(run_dir / "translation-differences.json"),
            "differences_csv": str(run_dir / "translation-differences.csv"),
        },
        "finished_utc": datetime.now(timezone.utc).isoformat(),
    }
    (run_dir / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps({"report": str(run_dir / "report.json"), "models": model_summaries,
                      "translation_difference_count": len(differences)}, ensure_ascii=False))


if __name__ == "__main__":
    main()

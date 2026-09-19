"""Read-only structure gate for a fixed OCR/cache set.

The report deliberately measures structure independently of model output.
"""
from __future__ import annotations
import argparse
import json
from pathlib import Path
import sys
import time
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from backend.structure import ensure_dialogues


def page_report(path: Path, image: Path | None = None):
    try:
        page = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        return {"page": path.name, "valid": False, "error": str(exc)}
    # Legacy benchmark caches are upgraded in memory only; the evaluator is
    # intentionally read-only and never rewrites the baseline.
    old_order = [str(b.get('id')) for b in sorted(page.get('blocks', []), key=lambda b: b.get('reading_order', 10**9))]
    started = time.perf_counter()
    ensure_dialogues(page, image=image)
    elapsed = time.perf_counter() - started
    blocks = page.get("blocks", [])
    dialogues = page.get("dialogues", [])
    ids = {str(b.get("id")) for b in blocks}
    seen = []
    duplicate_ids = set()
    for dialogue in dialogues:
        members = [str(x) for x in dialogue.get("block_ids", [])]
        seen.extend(members)
        duplicate_ids.update(str(b.get("id")) for b in blocks if b.get("duplicate_of"))
    missing = sorted(ids - set(seen))
    repeated = sorted({x for x in seen if seen.count(x) > 1})
    order = [int(b.get("reading_order")) for b in blocks
             if isinstance(b.get("reading_order"), int)]
    order_ok = sorted(order) == list(range(1, len(order) + 1)) and len(order) == len(blocks)
    region_members = {str(x) for r in page.get("overlay_regions", []) for x in r.get("block_ids", [])}
    return {
        "page": path.name, "valid": True, "ocr_signature": page.get("structure", {}).get("ocr_signature"),
        "blocks": len(blocks), "dialogues": len(dialogues),
        "assigned_blocks": len(set(seen)), "unassigned_blocks": len(missing),
        "duplicate_blocks": len(duplicate_ids), "repeated_members": repeated,
        "reading_order_contiguous": order_ok,
        "review_candidates": sum(bool(d.get("needs_review")) for d in dialogues),
        "failure_attribution": page.get("diagnostics", {}).get("failures", {}),
        "failure_units": page.get("diagnostics", {}).get("failure_units", {}),
        "translation_pending_dialogues": page.get("diagnostics", {}).get("translation", {}).get("pending_dialogues", 0),
        "region_count": len(page.get("overlay_regions", [])),
        "region_member_blocks": len(region_members),
        "safe_regions": sum(r.get("overlay") in {"balloon", "rectangle"}
                             for r in page.get("overlay_regions", [])),
        "missing_block_ids": missing,
        "missing_boxes": page.get('diagnostics', {}).get('ocr', {}).get('missing_box', 0),
        "panel_count": len(page.get('panels', [])),
        "panels": page.get('panels', []),
        "old_order": old_order,
        "new_order": [str(b.get('id')) for b in sorted(blocks, key=lambda b: b.get('reading_order', 10**9))],
        "structure_seconds": elapsed,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cache_dir", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--image-dir", type=Path, help='读取原图的分镜沟槽；省略时使用缓存或文字框顺序回退')
    args = parser.parse_args()
    paths = [args.cache_dir] if args.cache_dir.is_file() else sorted(args.cache_dir.glob("*.json"))
    if not paths:
        parser.error('没有找到页面缓存')
    images = {p.stem: p for p in args.image_dir.iterdir() if p.is_file()} if args.image_dir else {}
    if args.image_dir and any(p.stem not in images for p in paths):
        parser.error('图片目录中缺少对应缓存的页面')
    pages = [page_report(path, images.get(path.stem)) for path in paths]
    valid = [p for p in pages if p.get("valid")]
    failure_totals = {key: sum(p.get("failure_attribution", {}).get(key, 0) for p in valid)
                      for key in ("ocr", "grouping", "translation", "grouping_review")}
    failure_totals['layout'] = None  # This evaluator never invokes the renderer.
    report = {
        "cache_dir": str(args.cache_dir), "pages": len(pages), "valid_pages": len(valid),
        "ocr_signatures": sorted({p["ocr_signature"] for p in valid if p.get("ocr_signature")}),
        "blocks": sum(p.get("blocks", 0) for p in valid),
        "dialogues": sum(p.get("dialogues", 0) for p in valid),
        "unassigned_blocks": sum(p.get("unassigned_blocks", 0) for p in valid),
        "non_contiguous_order_pages": sum(not p.get("reading_order_contiguous") for p in valid),
        "review_candidates": sum(p.get("review_candidates", 0) for p in valid),
        "safe_regions": sum(p.get("safe_regions", 0) for p in valid),
        "panels": sum(p['panel_count'] for p in valid),
        "multiple_panel_pages": sum(p['panel_count'] > 1 for p in valid),
        "changed_order_pages": sum(p['old_order'] != p['new_order'] for p in valid),
        "structure_seconds": sum(p['structure_seconds'] for p in valid),
        "failure_attribution": failure_totals,
        "failure_units": valid[0].get('failure_units', {}) if valid else {},
        "layout_measured_pages": 0,
        "translation_pending_dialogues": sum(p['translation_pending_dialogues'] for p in valid),
        "limits": ['分镜属于候选，未检测到沟槽时退回几何顺序', '序号连续和分镜数量不等于排序准确率'],
        "page_reports": pages,
    }
    text = json.dumps(report, ensure_ascii=False, indent=2)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text, encoding="utf-8")
    print(text)


if __name__ == "__main__":
    main()

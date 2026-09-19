"""Versioned, inspectable OCR grouping; geometry proposals never authorize paint."""
from __future__ import annotations

import hashlib
import json
import math
import re
import unicodedata

ALGORITHM_VERSION = 5


def _digest(value):
    return hashlib.sha256(json.dumps(value, ensure_ascii=False, sort_keys=True).encode()).hexdigest()


def ocr_signature(page):
    return _digest([page.get("width"), page.get("height"), sorted([
        [str(b.get("id")), b.get("box"), b.get("source"), bool(b.get("vertical"))]
        for b in page.get("blocks", [])], key=lambda b: b[0])])


def structure_signature(page):
    # Translation-dependent local fill rectangles must not change dialogue units.
    regions = [{k: r.get(k) for k in ("id", "block_ids", "vertical", "points", "text_box")}
               for r in page.get("overlay_regions", []) if r.get("overlay_score", 1) > .5]
    return _digest([ALGORITHM_VERSION, ocr_signature(page), regions, page.get('panels', []),
                    [[str(b.get("id")), b.get("dialogue_group"), b.get("text_kind_override")]
                     for b in page.get("blocks", [])]])


def classify_text(source):
    value = unicodedata.normalize("NFKC", str(source or "")).strip()
    if not value:
        return "noise"
    if not re.search(r"[\w\u3040-\u30ff\u4e00-\u9fff]", value):
        return "utterance_candidate"
    if re.fullmatch(r"[ァ-ヶーッっぁぃぅぇぉ…！？!?,、。\s]+", value):
        return "sfx_candidate"
    if re.search(r"[A-Za-z]", value) and not re.search(r"[\u3040-\u30ff]", value):
        return "page_text_candidate"
    return "dialogue"


def _box(block):
    box = block.get("box")
    if not isinstance(box, (list, tuple)) or len(box) != 4:
        return None
    try:
        a, b, c, d = map(float, box)
        return [a, b, c, d] if all(map(math.isfinite, (a, b, c, d))) and c > a and d > b else None
    except (TypeError, ValueError):
        return None


def _union(boxes):
    return [min(b[0] for b in boxes), min(b[1] for b in boxes),
            max(b[2] for b in boxes), max(b[3] for b in boxes)]


def _center(box):
    return [(box[0]+box[2])/2, (box[1]+box[3])/2]


def _bands(items, axis):
    """Projection components: true whitespace separates rows/columns."""
    bands = []
    for item in sorted(items, key=lambda x: (x["box"][axis], str(x["id"]))):
        lo, hi = item["box"][axis], item["box"][axis+2]
        if not bands or lo >= bands[-1][1]:
            bands.append([lo, hi, [item]])
        else:
            bands[-1][1] = max(bands[-1][1], hi)
            bands[-1][2].append(item)
    return bands


def reading_order(items, vertical=False, page=False):
    if len(items) < 2:
        return items
    # Pages use horizontal gutters first, then Japanese right-to-left columns.
    # A detected page/panel is read as Japanese manga: rightmost column first,
    # then top-to-bottom within that column. A single horizontal dialogue still
    # uses its natural top-to-bottom line order below.
    axes = (0, 1) if page else ((0, 1) if vertical else (1, 0))
    for axis in axes:
        bands = _bands(items, axis)
        if len(bands) > 1:
            if axis == 0 and (page or vertical):
                bands.reverse()
            return [item for band in bands for item in reading_order(band[2], vertical, page)]
    return sorted(items, key=lambda x: ((-_center(x["box"])[0], _center(x["box"])[1])
                                       if vertical or page else (_center(x["box"])[1], _center(x["box"])[0])) + (str(x["id"]),))


def _duplicate(a, b):
    normalize = lambda s: re.sub(r"\s+", "", unicodedata.normalize("NFKC", str(s or "")))
    if not normalize(a.get("source")) or normalize(a.get("source")) != normalize(b.get("source")):
        return False
    x, y = a["box"], b["box"]
    area = max(0, min(x[2], y[2])-max(x[0], y[0])) * max(0, min(x[3], y[3])-max(x[1], y[1]))
    return area / min((x[2]-x[0])*(x[3]-x[1]), (y[2]-y[0])*(y[3]-y[1])) >= .85


def _near(a, b, vertical):
    x, y = a["box"], b["box"]
    along, across = (1, 0) if vertical else (0, 1)
    overlap = min(x[along+2], y[along+2])-max(x[along], y[along])
    length = min(x[along+2]-x[along], y[along+2]-y[along])
    thickness = min(x[across+2]-x[across], y[across+2]-y[across])
    gap = max(0, max(x[across], y[across])-min(x[across+2], y[across+2]))
    return overlap >= .55*length and gap <= max(4, .75*thickness)


def build_dialogues(page):
    blocks = page.get("blocks", [])
    old = {d.get("input_signature"): d for d in page.get("dialogues", [])}
    by_id = {str(b["id"]): b for b in blocks if b.get("id") is not None and _box(b)}
    panels = sorted(page.get('panels', []), key=lambda p: p['reading_order'])
    panel_by_block = {str(bid): p['id'] for p in panels for bid in p.get('block_ids', [])}
    for b in blocks:
        for key in ("dialogue_id", "dialogue_order", "reading_order", "duplicate_of", "panel_id"):
            b.pop(key, None)
        b["candidate_kind"] = classify_text(b.get("source"))
        b["text_kind"] = b.get("text_kind_override") or b["candidate_kind"]
    groups, used = [], set()

    def add(ids, method, region=None):
        buckets = {}
        for bid in ids:
            buckets.setdefault(panel_by_block.get(str(bid)), []).append(bid)
        for panel_id, members in buckets.items():
            add_panel_group(members, method, region, panel_id)

    def add_panel_group(ids, method, region, panel_id):
        ids = [str(i) for i in ids if str(i) in by_id and str(i) not in used]
        if not ids:
            return
        members = [by_id[i] for i in ids]
        vertical = sum(bool(b.get("vertical")) for b in members)*2 >= len(members)
        if region and "vertical" in region:
            vertical = bool(region["vertical"])
        groups.append({"id": min(ids), "block_ids": ids, "box": _union([b["box"] for b in members]),
                       "panel_id": panel_id,
                       "vertical": vertical, "grouping_method": method,
                       "overlay_region_id": str(region["id"]) if region else None})
        used.update(ids)

    # Explicit per-block keys allow one detected bubble to contain separate speakers.
    manual = {}
    for bid, b in by_id.items():
        if b.get("dialogue_group"):
            manual.setdefault(str(b["dialogue_group"]), []).append(bid)
    for ids in manual.values():
        add(ids, "manual")
    for region in sorted(page.get("overlay_regions", []), key=lambda r: (-len(r.get("block_ids", [])), str(r.get("id")))):
        if region.get("overlay_score", 1) > .5:
            add(region.get("block_ids", []), "region", region)
    remaining = sorted((b for bid, b in by_id.items() if bid not in used), key=lambda b: str(b["id"]))
    for seed in remaining:
        if str(seed["id"]) in used:
            continue
        members = [seed]
        for b in remaining:
            if b is seed or str(b["id"]) in used:
                continue
            if panel_by_block.get(str(b['id'])) != panel_by_block.get(str(seed['id'])):
                continue
            # All-pairs test prevents a chain of nearby boxes bridging two bubbles.
            if bool(b.get("vertical")) == bool(seed.get("vertical")) and all(
                    _duplicate(a, b) or (a["text_kind"] == b["text_kind"] and _near(a, b, bool(seed.get("vertical"))))
                    for a in members):
                members.append(b)
        add([b["id"] for b in members], "geometry_candidate" if len(members) > 1 else "singleton")
    dialogues = []
    ordinal = 0
    panel_order = [p['id'] for p in panels] + [None]
    ordered_groups = [g for pid in panel_order for g in reading_order(
        [g for g in groups if g['panel_id'] == pid], page=True)]
    for index, group in enumerate(ordered_groups, 1):
        members = reading_order([by_id[i] for i in group["block_ids"]], group["vertical"])
        canonical = []
        for b in members:
            match = next((a for a in canonical if _duplicate(a, b) and not a.get("edited") and not b.get("edited")), None)
            if match:
                b["duplicate_of"] = str(match["id"])
            else:
                canonical.append(b)
        ids = [str(b["id"]) for b in members]
        source_ids = [str(b["id"]) for b in canonical]
        signature = _digest([[i, by_id[i].get("source"), by_id[i]["box"]] for i in ids])
        dialogue = {**group, "id": f"d{index}", "block_ids": ids, "source_block_ids": source_ids,
                    "source": "\n".join(str(b.get("source", "")).strip() for b in canonical),
                    "input_signature": signature,
                    "needs_review": group["grouping_method"] in {"geometry_candidate", "singleton"}}
        previous = old.get(signature, {})
        for key in ("full_translation", "mapping", "mapping_status", "translation_error", "translation_config", "quality_warnings"):
            if key in previous:
                dialogue[key] = previous[key]
        for order, b in enumerate(members, 1):
            ordinal += 1
            b.update(dialogue_id=dialogue["id"], dialogue_order=order, reading_order=ordinal, panel_id=group['panel_id'])
        dialogues.append(dialogue)
    page["dialogues"] = dialogues
    page["structure"] = {"version": ALGORITHM_VERSION, "signature": structure_signature(page),
                         "ocr_signature": ocr_signature(page), "reading_order": [d["id"] for d in dialogues],
                         "block_count": len(blocks), "dialogue_count": len(dialogues),
                         "order_method": "image panel gutters, then bubble geometry and internal direction" if panels else "OCR geometry fallback",
                         "panel_order": [p['id'] for p in panels]}
    page["diagnostics"] = {
        "panels": {"count": len(panels), "unassigned_blocks": sum(str(b.get('id')) not in panel_by_block for b in blocks),
                   "reason": page.get('panel_error', '') if panels else page.get('panel_error', '未确认图像分镜，使用文字框顺序')},
        "ocr": {"blocks": len(blocks), "missing_box": sum(_box(b) is None for b in blocks),
                "duplicates": sum(bool(b.get("duplicate_of")) for b in blocks)},
        "grouping": {"dialogues": len(dialogues), "panels": len(panels), "unassigned": sum("dialogue_id" not in b for b in blocks),
                     "review_candidates": sum(d["needs_review"] for d in dialogues)},
        "text_kinds": {kind: sum(b["text_kind"] == kind for b in blocks) for kind in sorted({b["text_kind"] for b in blocks})}}
    sync_dialogue_translations(page)
    return dialogues


def ensure_dialogues(page, image=None, gray=None):
    if image is not None or gray is not None or 'panel_signature' in page:
        try:
            from backend.panels import ensure_panels
        except ImportError:  # worker.py can also load this module directly
            from panels import ensure_panels
        ensure_panels(page, gray=gray, image=image)
    if page.get("structure", {}).get("signature") != structure_signature(page):
        return build_dialogues(page)
    sync_dialogue_translations(page)
    return page.get("dialogues", [])


def valid_block(b):
    text = str(b.get("translation") or "").strip()
    return bool(text) and (bool(b.get("edited")) or (not b.get("error") and text != str(b.get("source") or "").strip()))


def sync_dialogue_translations(page):
    by_id = {str(b.get("id")): b for b in page.get("blocks", [])}
    for b in by_id.values():
        b.pop("layout_fallback", None)
        for key in ("dialogue_translation", "dialogue_translation_complete", "dialogue_member_ids"):
            b.pop(key, None)
    for d in page.get("dialogues", []):
        members = [by_id[i] for i in d.get("source_block_ids", d.get("block_ids", [])) if i in by_id]
        all_members = [by_id[i] for i in d.get("block_ids", []) if i in by_id]
        mapping = d.get("mapping", {})
        full_current = (valid_block({"source": d.get("source"), "translation": d.get("full_translation"),
                                     "error": d.get("translation_error")}) and bool(members)
                        and len(all_members) == len(d.get("block_ids", []))
                        and set(mapping) == {str(b["id"]) for b in members}
                        and all(not b.get("edited") and b.get("overlay") != "skip" for b in all_members)
                        and all(not b.get("error") and mapping[str(b["id"])] == b.get("translation", "") for b in members))
        d["translation_complete"] = full_current or bool(members) and all(valid_block(b) for b in members)
        d["translation"] = d["full_translation"] if full_current else "\n".join(str(b.get("translation") or "").strip() for b in members)
        for b in all_members:
            b["dialogue_translation"] = d["full_translation"] if full_current else ""
            b["dialogue_translation_complete"] = bool(full_current)
            b["dialogue_member_ids"] = list(d.get("block_ids", []))
            if full_current and b.get("duplicate_of") in by_id:
                origin = by_id[b["duplicate_of"]]
                b["translation"], b["error"] = origin.get("translation", ""), origin.get("error", "")
        d["translation_mode"] = "dialogue" if full_current else "blocks"
        d.setdefault("quality_warnings", [])
        if not d["translation_complete"]:
            for i in d.get("block_ids", []):
                if i in by_id:
                    by_id[i]["layout_fallback"] = "block"
    diag = page.setdefault("diagnostics", {})
    diag["translation"] = {"translated_blocks": sum(valid_block(b) for b in by_id.values()),
                            "errors": sum(bool(b.get("error")) for b in by_id.values()),
                            "complete_dialogues": sum(d["translation_complete"] for d in page.get("dialogues", [])),
                            "quality_warnings": sum(len(b.get("quality_warnings", [])) for b in by_id.values()),
                            "dialogue_quality_warnings": sum(len(d.get("quality_warnings", [])) for d in page.get("dialogues", []))}
    # Structure/translation synchronization does not run the renderer. Missing
    # translations are prerequisites, not observed font-fitting failures.
    diag["layout"] = {"status": "not_measured", "failed_regions": None,
                      "uncertain_blocks": sum(b.get("overlay") == "uncertain" for b in by_id.values()),
                      "safe_regions": sum(r.get("overlay") in {"balloon", "rectangle"} for r in page.get("overlay_regions", [])),
                      "fallback_dialogues": sum(not d["translation_complete"] for d in page.get("dialogues", []))}
    failed_dialogues = [d for d in page.get("dialogues", []) if d.get("translation_error") or
                        any(by_id.get(str(i), {}).get("error") for i in d.get("block_ids", []))]
    assigned_ids = {str(i) for d in page.get("dialogues", []) for i in d.get("block_ids", [])}
    diag["translation"]["failed_dialogues"] = len(failed_dialogues)
    diag["translation"]["pending_dialogues"] = sum(
        not d.get("translation_complete") and d not in failed_dialogues for d in page.get("dialogues", []))
    diag["failures"] = {
        "ocr": sum(not str(b.get("source") or "").strip() or _box(b) is None for b in by_id.values()),
        "grouping": sum(not d.get("block_ids") for d in page.get("dialogues", [])) + page.get("diagnostics", {}).get("grouping", {}).get("unassigned", 0),
        "translation": len(failed_dialogues) + sum(bool(b.get("error")) for bid, b in by_id.items() if bid not in assigned_ids),
        "layout": None,
        "grouping_review": sum(bool(d.get("needs_review")) for d in page.get("dialogues", [])),
    }
    diag["failure_units"] = {"ocr": "blocks with empty source or invalid box; not OCR accuracy",
                             "grouping": "empty units plus unassigned blocks",
                             "translation": "failed dialogue units plus unassigned error blocks, deduplicated",
                             "layout": "unmeasured until a renderer attempts layout",
                             "grouping_review": "review candidates, not confirmed failures"}

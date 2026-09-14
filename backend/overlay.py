"""Conservative image backed overlay classification shared by worker and UI."""
from __future__ import annotations

import hashlib
import json
import unicodedata
from pathlib import Path

ALGORITHM_VERSION = 8
VALID = {"balloon", "rectangle", "skip", "uncertain"}
LOW_IMPACT = {"ん", "んん", "え", "えっ", "あ", "あっ", "お", "へえ", "うん", "まあ", "ふっ", "はあ", "ふふ"}


def is_low_impact_utterance(source, box=None, region=None):
    if region is not None or not isinstance(source, str) or not source.strip():
        return False
    value = unicodedata.normalize("NFKC", source).strip()
    compact = "".join(c for c in value if not c.isspace() and not unicodedata.category(c).startswith(("P", "S")))
    if not compact:
        return len(value) <= 8 and all(c.isspace() or c in ".!?…‥！？。" for c in value)
    return len(compact) <= 4 and compact in LOW_IMPACT


def _box(block, width, height):
    values = block.get("box")
    if not isinstance(values, (list, tuple)) or len(values) != 4:
        return None
    try:
        x1, y1, x2, y2 = [int(round(float(x))) for x in values]
    except (TypeError, ValueError, OverflowError):
        return None
    x1, y1, x2, y2 = max(0, x1), max(0, y1), min(width, x2), min(height, y2)
    return (x1, y1, x2, y2) if x2 - x1 >= 4 and y2 - y1 >= 4 else None


def _inner_rectangle(mask):
    import cv2
    import numpy as np
    distance = cv2.distanceTransform(np.pad(mask, 1), cv2.DIST_L2, 5)[1:-1, 1:-1]
    safe = distance >= max(3, min(mask.shape) * .04)
    heights = [0] * (mask.shape[1] + 1)
    best_area, best = 0, None
    for y, row in enumerate(safe):
        for x, inside in enumerate(row):
            heights[x] = heights[x] + 1 if inside else 0
        stack = []
        for x, value in enumerate(heights):
            start = x
            while stack and stack[-1][1] > value:
                left, h = stack.pop()
                area = (x - left) * h
                if area > best_area and x - left >= 12 and h >= 12:
                    best_area, best = area, (left, y-h+1, x, y+1)
                start = left
            if not stack or stack[-1][1] < value:
                stack.append((start, value))
    return best


def _coverage(candidate, box):
    if box is None:
        return 0.0
    x, y = candidate["origin"]
    mask = candidate["mask"]
    a, b, c, d = box
    left, top = max(x, a), max(y, b)
    right, bottom = min(x+mask.shape[1], c), min(y+mask.shape[0], d)
    if right <= left or bottom <= top:
        return 0.0
    return float(mask[top-y:bottom-y, left-x:right-x].sum()) / ((c-a)*(d-b))


def _component_candidate(gray, box):
    """Return one bounded bright component containing the OCR rectangle."""
    import cv2
    import numpy as np

    height, width = gray.shape[:2]
    x1, y1, x2, y2 = box
    bw, bh = x2 - x1, y2 - y1
    margin_x = min(int(width * .22), max(28, int(bw * 1.55)))
    margin_y = min(int(height * .22), max(28, int(bh * 1.55)))
    left, top = max(0, x1 - margin_x), max(0, y1 - margin_y)
    right, bottom = min(width, x2 + margin_x), min(height, y2 + margin_y)
    crop = gray[top:bottom, left:right]
    if crop.size == 0:
        return None
    # A bright mask is kept unclosed. Closing white pixels was the source of
    # the old bug: it bridged thin balloon outlines into page backgrounds.
    threshold = int(max(185, min(238, float(np.percentile(crop, 72)) - 8)))
    bright = cv2.inRange(crop, threshold, 255)
    count, labels, stats, _ = cv2.connectedComponentsWithStats(bright, 8)
    local_box = (x1 - left, y1 - top, x2 - left, y2 - top)
    candidates = []
    for label in range(1, count):
        x, y, w, h, pixels = [int(v) for v in stats[label]]
        if pixels < max(30, int(bw * bh * .22)):
            continue
        inside = labels[local_box[1]:local_box[3], local_box[0]:local_box[2]] == label
        coverage = float(inside.mean()) if inside.size else 0.0
        if coverage < .58:
            continue
        # A valid bubble cannot consume the search crop or grow many times
        # farther than the text. This rejects panel backgrounds and borders.
        if x <= 0 or y <= 0 or x + w >= crop.shape[1] - 1 or y + h >= crop.shape[0] - 1:
            continue
        ratio = pixels / max(1, bw * bh)
        bbox_ratio = (w * h) / max(1, bw * bh)
        # A real balloon may be a little larger than the detected text, but
        # panel-sized white areas must never become an eraser.  The rendered
        # mask uses the whole component bbox, so constrain that bbox too.
        if ratio > 8.0 or bbox_ratio > 4.0 or w > bw * 2.6 or h > bh * 2.6:
            continue
        component = (labels[y:y+h, x:x+w] == label).astype(np.uint8)
        contours, _ = cv2.findContours(component, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
        if not contours:
            continue
        contour = max(contours, key=cv2.contourArea)
        contour_area = float(cv2.contourArea(contour))
        if contour_area < bw * bh * .18:
            continue
        fill = min(1.0, contour_area / max(1.0, w * h))
        # Estimate fill and ink just outside the selected component. A panel
        # background has little edge contrast; a balloon has a dark outline.
        filled = np.zeros_like(component)
        cv2.drawContours(filled, [contour], -1, 1, cv2.FILLED)
        padded = np.pad(filled, 2)
        ring_mask = cv2.dilate(padded, np.ones((5, 5), np.uint8)) - padded
        patch = cv2.copyMakeBorder(gray[top+y:top+y+h, left+x:left+x+w], 2, 2, 2, 2, cv2.BORDER_REPLICATE)
        # Include actual exterior pixels, rather than the printed letters
        # inside a balloon, when assessing its outline contrast.
        if top+y >= 2 and left+x >= 2 and top+y+h+2 <= height and left+x+w+2 <= width:
            patch = gray[top+y-2:top+y+h+2, left+x-2:left+x+w+2]
        ring_pixels = patch[ring_mask > 0]
        interior_pixels = gray[top + y:top + y + h, left + x:left + x + w][component > 0]
        if interior_pixels.size < 20 or ring_pixels.size < 20:
            continue
        interior = float(np.median(interior_pixels))
        ring = float(np.percentile(ring_pixels, 25))
        contrast = max(0.0, min(1.0, (interior - ring) / 90.0))
        score = (.40 * coverage + .22 * fill + .20 * contrast +
                 .18 * max(0.0, 1.0 - ratio / 8.0))
        if contrast < .45 or score < .72:
            continue
        points = contour.reshape(-1, 2)
        if len(points) < 3:
            continue
        inner = _inner_rectangle(filled)
        if inner is None:
            continue
        candidates.append({
            "score": float(score),
            "points": [[int(px + x + left), int(py + y + top)] for px, py in points],
            "text_box": [x + left + inner[0], y + top + inner[1],
                         x + left + inner[2], y + top + inner[3]],
            "fill_color": [int(round(interior))] * 3,
            "mask": filled, "origin": (x + left, y + top),
            "area": int(filled.sum()), "members": [],
        })
    return max(candidates, key=lambda item: item["score"]) if candidates else None


def _local_rectangle(gray, box):
    """Small OCR-centered fill for uniform light panels without a border."""
    import cv2
    import numpy as np
    x1, y1, x2, y2 = box
    bw, bh = x2 - x1, y2 - y1
    # Fallback panels are deliberately tight: the OCR box is the only solid
    # evidence we have when no closed balloon was found.
    pad_x, pad_y = max(4, int(bw * .08)), max(4, int(bh * .06))
    left, top = max(0, x1 - pad_x), max(0, y1 - pad_y)
    right, bottom = min(gray.shape[1], x2 + pad_x), min(gray.shape[0], y2 + pad_y)
    patch = gray[top:bottom, left:right]
    if patch.size < 100:
        return None
    median, spread = float(np.median(patch)), float(np.percentile(patch, 90) - np.percentile(patch, 10))
    dark_fraction = float((patch < median - 55).mean())
    if median < 190 or spread > 70 or dark_fraction < .008:
        return None
    # Keep the rectangle tightly tied to the OCR bounds; never use a whole
    # connected component or panel-sized expansion here.
    return {"score": .52, "points": [[left, top], [right, top], [right, bottom], [left, bottom]],
            "text_box": [left + max(3, pad_x * .45), top + max(3, pad_y * .45),
                         right - max(3, pad_x * .45), bottom - max(3, pad_y * .45)],
            "fill_color": [int(round(median))] * 3,
            "mask": np.ones((bottom-top, right-left), np.uint8), "origin": (left, top),
            "area": (right-left) * (bottom-top), "members": []}


def _mask_overlap(first, second):
    x, y = first["origin"]
    a, b = second["origin"]
    m, n = first["mask"], second["mask"]
    left, top, right, bottom = max(x, a), max(y, b), min(x + m.shape[1], a + n.shape[1]), min(y + m.shape[0], b + n.shape[0])
    if right <= left or bottom <= top:
        return 0.0
    common = ((m[top-y:bottom-y, left-x:right-x] > 0) & (n[top-b:bottom-b, left-a:right-a] > 0)).sum()
    return float(common) / max(1, min(first["area"], second["area"]))


def _union_box(boxes):
    return (min(b[0] for b in boxes), min(b[1] for b in boxes),
            max(b[2] for b in boxes), max(b[3] for b in boxes))


def _adjacent_text(gray, first, second, vertical):
    """Allow a joint search only across a short blank gap, never a panel line."""
    import numpy as np
    a, b = first, second
    if vertical:
        overlap = min(a[2], b[2]) - max(a[0], b[0])
        if overlap > .5 * min(a[2]-a[0], b[2]-b[0]):
            a, b = sorted((a, b), key=lambda box: box[1])
            gap = b[1] - a[3]
            strip = gray[a[3]:b[1], max(a[0], b[0]):min(a[2], b[2])]
            limit = .55 * min(a[2]-a[0], b[2]-b[0])
            barrier_axis = 1
        else:
            a, b = sorted((a, b), key=lambda box: box[0])
            if min(a[3], b[3])-max(a[1], b[1]) < .6*min(a[3]-a[1], b[3]-b[1]):
                return False
            gap = b[0] - a[2]
            strip = gray[max(a[1], b[1]):min(a[3], b[3]), a[2]:b[0]]
            limit = .6 * max(a[2]-a[0], b[2]-b[0])
            barrier_axis = 0
    else:
        a, b = sorted((a, b), key=lambda box: box[1])
        if min(a[2], b[2])-max(a[0], b[0]) < .6*min(a[2]-a[0], b[2]-b[0]):
            return False
        gap = b[1] - a[3]
        strip = gray[a[3]:b[1], max(a[0], b[0]):min(a[2], b[2])]
        limit = .6 * max(a[3]-a[1], b[3]-b[1])
        barrier_axis = 1
    if not 0 <= gap <= max(6, limit):
        return False
    if gap == 0:
        return True
    if not strip.size:
        return False
    # A continuation stroke may run through the OCR gap. Reject lines
    # spanning across the gap, while tolerating sparse glyph ink in it.
    return (float(np.mean(strip >= 220)) >= .8 and
            not np.any(np.mean(strip < 130, axis=barrier_axis) >= .65))


def _erase_polygons(candidate, member_boxes):
    """Erase the detected text, while keeping the rest of the balloon intact."""
    import cv2
    import numpy as np
    x, y = candidate["origin"]
    mask = np.zeros_like(candidate["mask"])
    for x1, y1, x2, y2 in member_boxes:
        pad = max(3, min(7, round(min(x2-x1, y2-y1) * .08)))
        left, top = max(0, x1-x-pad), max(0, y1-y-pad)
        right, bottom = min(mask.shape[1], x2-x+pad), min(mask.shape[0], y2-y+pad)
        mask[top:bottom, left:right] = 1
    mask &= candidate["mask"]
    contours, _ = cv2.findContours(mask, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
    return [[[int(px+x), int(py+y)] for px, py in contour.reshape(-1, 2)]
            for contour in contours if len(contour) >= 3]


def ensure_page_overlays(page, gray=None, image=None):
    """Recompute safe overlay regions, preserving OCR and manual translations."""
    import cv2
    import numpy as np
    if gray is None and image is not None:
        try:
            gray = cv2.imdecode(np.frombuffer(Path(image).read_bytes(), np.uint8), cv2.IMREAD_GRAYSCALE)
        except (OSError, ValueError, TypeError, cv2.error):
            gray = None
    if gray is not None and (getattr(gray, "ndim", 0) != 2 or not gray.size):
        gray = None
    blocks = page.get("blocks", [])
    # Fallback regions depend on whether a translation is ready. Without
    # this state in the signature, the first OCR-only pass cached uncertain
    # forever and the later translated pass never gained a visible fill.
    signature_data = [[b.get("id"), b.get("box"), b.get("source"), b.get("vertical"),
                       bool(str(b.get("translation") or "").strip()), bool(b.get("error")),
                       bool(b.get("edited"))] for b in blocks]
    fingerprint = hashlib.sha256(json.dumps(signature_data, ensure_ascii=False).encode()).hexdigest()
    image_key = hashlib.sha256(gray.tobytes()).hexdigest() if gray is not None else None
    signature = [ALGORITHM_VERSION, fingerprint, image_key]
    if page.get("overlay_signature") == signature and "overlay_regions" in page:
        return False
    for block in blocks:
        block.update(overlay="uncertain", overlay_score=0.0,
                     overlay_reason="未确认封闭且有明显边界的对话框，保留原图", overlay_region_id=None)
    boxes = [_box(block, *gray.shape[:2][::-1]) if gray is not None else None for block in blocks]
    candidates = []
    if gray is not None:
        for index, box in enumerate(boxes):
            if box is None:
                continue
            candidate = _component_candidate(gray, box)
            if candidate is None and not is_low_impact_utterance(blocks[index].get("source")):
                candidate = _local_rectangle(gray, box)
            if candidate:
                coverage = [_coverage(candidate, other) for other in boxes]
                members = [i for i, value in enumerate(coverage) if value >= .94]
                if index not in members or any(.15 < value < .94 for value in coverage):
                    continue
                text_area = sum((boxes[i][2]-boxes[i][0])*(boxes[i][3]-boxes[i][1]) for i in members)
                bbox_area = candidate["mask"].size
                if bbox_area > 3.5 * text_area:
                    continue
                candidate["members"] = members
                candidate["overlay"] = "balloon" if candidate["score"] >= .72 else "rectangle"
                candidates.append(candidate)
        # A fragmented sentence can be too small to justify its full balloon
        # alone. A joint search still has to recover one closed component
        # containing every member, so proximity alone never creates a group.
        for first, first_box in enumerate(boxes):
            if first_box is None:
                continue
            for second in range(first+1, len(boxes)):
                second_box = boxes[second]
                vertical = bool(blocks[first].get("vertical"))
                if second_box is None or vertical != bool(blocks[second].get("vertical")):
                    continue
                if not _adjacent_text(gray, first_box, second_box, vertical):
                    continue
                joined_box = _union_box([first_box, second_box])
                candidate = _component_candidate(gray, joined_box)
                if not candidate:
                    continue
                coverage = [_coverage(candidate, box) for box in boxes]
                members = [i for i, value in enumerate(coverage) if value >= .94]
                if first not in members or second not in members or any(.15 < v < .94 for v in coverage):
                    continue
                bounds = _union_box([boxes[i] for i in members])
                if candidate["mask"].size > 4 * (bounds[2]-bounds[0]) * (bounds[3]-bounds[1]):
                    continue
                candidate.update(members=members, overlay="balloon")
                candidates.append(candidate)
    regions = []
    accepted_candidates = []
    assigned = set()
    for candidate in sorted(candidates, key=lambda item: (-len(item["members"]), -item["score"])):
        if assigned.intersection(candidate["members"]) or any(_mask_overlap(candidate, previous) > .50 for previous in accepted_candidates):
            continue
        member_indexes = list(candidate["members"])
        member_indexes = sorted(set(member_indexes))
        region_id = str(len(regions) + 1)
        overlay = candidate.get("overlay", "balloon")
        reason = "已确认封闭边界，使用气泡内部轮廓" if overlay == "balloon" else "背景均匀，使用限于文字框的小范围填充"
        region = {"id": region_id, "block_ids": [str(blocks[i].get("id")) for i in member_indexes],
                  "overlay": overlay, "overlay_score": candidate["score"], "overlay_reason": reason,
                  "points": candidate["points"], "text_box": candidate["text_box"],
                  "erase_points": _erase_polygons(candidate, [boxes[i] for i in member_indexes]),
                  "fill_color": candidate["fill_color"], "vertical": all(bool(blocks[i].get("vertical")) for i in member_indexes)}
        regions.append(region)
        accepted_candidates.append(candidate)
        assigned.update(member_indexes)
        for i in member_indexes:
            blocks[i].update(overlay=overlay, overlay_score=candidate["score"], overlay_reason=reason, overlay_region_id=region_id)
    for index, block in enumerate(blocks):
        if gray is not None and boxes[index] and index not in assigned and is_low_impact_utterance(block.get("source")):
            block.update(overlay="skip", overlay_score=0.0, overlay_reason="无可靠气泡背景的短语气词，保留原图", overlay_region_id=None)
        elif gray is not None and boxes[index] and index not in assigned:
            # Translated text without a recoverable balloon may get a tiny,
            # OCR-centered fill only when the surrounding patch is genuinely
            # light. This keeps image text (for example lettering on a car)
            # untouched instead of painting a white card over it.
            text = str(block.get("translation") or "").strip()
            if text and (not block.get("error") or block.get("edited")):
                x1, y1, x2, y2 = boxes[index]
                px, py = max(4, int((x2-x1) * .08)), max(4, int((y2-y1) * .06))
                left, top = max(0, x1-px), max(0, y1-py)
                right, bottom = min(gray.shape[1], x2+px), min(gray.shape[0], y2+py)
                patch = gray[top:bottom, left:right]
                if patch.size == 0:
                    continue
                # Dark printed glyphs are expected, but most pixels around a
                # regular text panel should still be near-white. A mixed
                # scene background fails this test and remains uncertain.
                light_fraction = float(np.mean(patch >= 220))
                light_pixels = patch[patch >= 220]
                if light_pixels.size == 0 or light_fraction < .68:
                    continue
                fill_median = float(np.median(light_pixels))
                fill_color = [int(round(fill_median))] * 3
                candidate = {"origin": (left, top), "mask": np.ones(patch.shape, np.uint8),
                             "area": patch.size}
                if any(_mask_overlap(candidate, previous) > .02 for previous in accepted_candidates):
                    continue
                if any(i != index and _coverage(candidate, other) > .1 for i, other in enumerate(boxes)):
                    continue
                region_id = str(len(regions) + 1)
                reason = "未确认气泡边界，使用限制在文字框附近的小范围填充"
                region = {"id": region_id, "block_ids": [str(block.get("id"))],
                          "overlay": "rectangle", "overlay_score": .50, "overlay_reason": reason,
                          "points": [[left, top], [right, top], [right, bottom], [left, bottom]],
                          "text_box": [left+px*.35, top+py*.35, right-px*.35, bottom-py*.35],
                          "fill_color": fill_color, "vertical": bool(block.get("vertical"))}
                regions.append(region)
                accepted_candidates.append(candidate)
                block.update(overlay="rectangle", overlay_score=.50, overlay_reason=reason, overlay_region_id=region_id)
    page.update(version=max(2, int(page.get("version", 0) or 0)), overlay_regions=regions, overlay_signature=signature)
    return True

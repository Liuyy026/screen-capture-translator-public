"""Read-only, image-backed display geometry for the native macOS reader.

The OCR cache is an input, never an output.  No OCR engines, model services or
translation workers are imported.  Unlike the shared conservative eraser, a
narrow vertical OCR column may use its entire *confirmed* balloon for layout;
only the original OCR ink bounds are erased.
"""
from __future__ import annotations

import argparse
import copy
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
from backend.overlay import _box, _coverage, _erase_polygons, _inner_rectangle, _mask_overlap
from macos.backend.reading_background import background_patch

VERSION = 2


def _ready(block):
    translation = block.get("translation")
    return isinstance(translation, str) and bool(translation.strip()) and not block.get("error")


def _members(candidate, boxes):
    coverage = [_coverage(candidate, box) for box in boxes]
    # An OCR rectangle straddling the contour is not safe to erase or regroup.
    if any(.02 < value < .94 for value in coverage):
        return []
    return [index for index, value in enumerate(coverage) if value >= .94]


def _shape(mask, origin):
    import cv2
    import numpy as np

    contours, _ = cv2.findContours(mask, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
    if not contours:
        return None
    contour = max(contours, key=cv2.contourArea)
    if len(contour) < 3:
        return None
    filled = np.zeros_like(mask)
    cv2.drawContours(filled, [contour], -1, 1, cv2.FILLED)
    x, y = origin
    return {"origin": origin, "mask": filled, "area": int(filled.sum()),
            "points": [[int(px+x), int(py+y)] for px, py in contour.reshape(-1, 2)],
            "contour": contour}


def _bounded_bubble(gray, labels, stats, label, boxes):
    import cv2
    import numpy as np

    height, width = gray.shape
    x, y, w, h, pixels = map(int, stats[label])
    # Reject page backgrounds, large panels, tiny glyph interiors and anything
    # connected to the page edge.  These bounds do not depend on OCR width.
    if (min(w, h) < 24 or pixels < 200 or x < 4 or y < 4
            or x+w > width-4 or y+h > height-4
            or w > width*.48 or h > height*.43
            or pixels > width*height*.085 or w*h > width*height*.14):
        return None
    component = (labels[y:y+h, x:x+w] == label).astype(np.uint8)
    candidate = _shape(component, (x, y))
    if candidate is None:
        return None
    filled, contour = candidate["mask"], candidate["contour"]
    contour_area = cv2.contourArea(contour)
    hull_area = cv2.contourArea(cv2.convexHull(contour))
    fill = candidate["area"] / (w*h)
    solidity = contour_area / max(1, hull_area)
    # A rectangular white panel with an outline is not evidence of a balloon.
    if not .42 <= fill <= .93 or solidity < .72:
        return None
    members = _members(candidate, boxes)
    if not members:
        return None
    if pixels / max(1, candidate["area"]) < .64:
        return None

    patch = gray[y:y+h, x:x+w]
    background = filled.copy()
    for index in members:
        a, b, c, d = boxes[index]
        background[max(0, b-y-7):min(h, d-y+7),
                   max(0, a-x-7):min(w, c-x+7)] = 0
    background_pixels = patch[background > 0]
    if background_pixels.size < 100:
        return None
    interior = float(np.median(background_pixels))
    # The text area must be blank outside known OCR ink.  Filling holes in a
    # connected component must not silently turn eyes, hair or scenery white.
    if (interior < 220 or np.mean(background_pixels >= 210) < .94
            or np.mean(background_pixels < interior-65) > .025):
        return None
    padded = np.pad(filled, 3)
    ring = cv2.dilate(padded, np.ones((7, 7), np.uint8))-padded
    ring_pixels = gray[y-3:y+h+3, x-3:x+w+3][ring > 0]
    if (ring_pixels.size < 40 or np.mean(ring_pixels < interior-55) < .45
            or interior-float(np.percentile(ring_pixels, 35)) < 60):
        return None
    candidate.update(members=members, fill_color=[int(round(interior))]*3,
                     solidity=solidity, score=.9)
    return candidate


def _split_lobes(candidate, boxes):
    """Separate joined balloons only when their filled shape has a thin neck.

    Erosion is applied *after* confirming the original closed contour.  It
    never seals a broken outline or connects the balloon to a background.
    Two OCR columns inside one oval stay together because its interior has
    one connected core.  Nearest-core partitioning only divides the blank
    neck; original boundaries and OCR erasure limits remain unchanged.
    """
    import cv2
    import numpy as np

    if len(candidate["members"]) < 2 or candidate["solidity"] >= .94:
        return [candidate]
    mask = candidate["mask"]
    distance = cv2.distanceTransform(np.pad(mask, 1), cv2.DIST_L2, 5)[1:-1, 1:-1]
    radius = min(24, max(5, min(mask.shape)*.085))
    cores = (distance > radius).astype(np.uint8)
    count, core_labels, stats, _ = cv2.connectedComponentsWithStats(cores, 8)
    sizeable = [i for i in range(1, count) if stats[i, cv2.CC_STAT_AREA] >= candidate["area"]*.12]
    if len(sizeable) < 2:
        return [candidate]
    cores = np.isin(core_labels, sizeable).astype(np.uint8)
    _, nearest = cv2.distanceTransformWithLabels(1-cores, cv2.DIST_L2, 5,
                                                labelType=cv2.DIST_LABEL_CCOMP)
    pieces = []
    assigned = set()
    for label in sorted(set(int(value) for value in nearest[cores > 0])):
        piece = _shape(((nearest == label) & (mask > 0)).astype(np.uint8), candidate["origin"])
        if piece is None:
            return [candidate]
        members = _members(piece, boxes)
        if not members or assigned.intersection(members):
            return [candidate]
        piece.update(members=members, fill_color=candidate["fill_color"], score=candidate["score"])
        pieces.append(piece)
        assigned.update(members)
    return pieces if assigned == set(candidate["members"]) else [candidate]


def _cached_shape(region, width, height):
    """Small bounded raster used solely to reject overlapping old fallbacks."""
    import cv2
    import numpy as np

    try:
        points = np.asarray(region.get("points"), dtype=np.float64)
        if (points.ndim != 2 or points.shape[1] != 2 or len(points) < 3
                or not np.isfinite(points).all()
                or points[:, 0].min() < 0 or points[:, 0].max() > width
                or points[:, 1].min() < 0 or points[:, 1].max() > height):
            return None
        points = np.rint(points).astype(np.int32)
        x, y, w, h = cv2.boundingRect(points)
        if w*h > width*height*.14:
            return None
        mask = np.zeros((h, w), np.uint8)
        cv2.fillPoly(mask, [points-[x, y]], 1)
        return {"origin": (x, y), "mask": mask, "area": int(mask.sum())}
    except (TypeError, ValueError, OverflowError, cv2.error):
        return None


def _outlined_bubbles(gray, boxes, existing):
    """Recover closed gray/halftone balloons from their actual dark outline."""
    import cv2
    import numpy as np

    height, width = gray.shape
    assigned = {index for candidate in existing for index in candidate["members"]}
    pending = [i for i, box in enumerate(boxes) if box and i not in assigned]
    if not pending:
        return []
    candidates = []
    for threshold in (95, 125, 155):
        contours, _ = cv2.findContours((gray < threshold).astype(np.uint8), cv2.RETR_LIST, cv2.CHAIN_APPROX_SIMPLE)
        for contour in contours:
            x, y, w, h = cv2.boundingRect(contour)
            area = cv2.contourArea(contour)
            if (min(w, h) < 30 or area < 500 or area > width*height*.085
                    or x < 4 or y < 4 or x+w > width-4 or y+h > height-4
                    or w > width*.48 or h > height*.43
                    or not .45 <= area/max(1, w*h) <= .93):
                continue
            if not any(x <= boxes[i][0] and y <= boxes[i][1]
                       and x+w >= boxes[i][2] and y+h >= boxes[i][3] for i in pending):
                continue
            solidity = area/max(1, cv2.contourArea(cv2.convexHull(contour)))
            if solidity < .82:
                continue
            mask = np.zeros((h, w), np.uint8)
            cv2.drawContours(mask, [contour-[x, y]], -1, 1, cv2.FILLED)
            candidate = _shape(mask, (x, y))
            members = _members(candidate, boxes)
            if not members or assigned.intersection(members):
                continue
            background = cv2.erode(mask, np.ones((9, 9), np.uint8))
            for i in members:
                a, b, c, d = boxes[i]
                background[max(0, b-y-10):min(h, d-y+10), max(0, a-x-10):min(w, c-x+10)] = 0
            if background.sum() < 300:
                continue
            patch = gray[y:y+h, x:x+w]
            values = patch[background > 0]
            smooth = cv2.GaussianBlur(patch, (9, 9), 2)[background > 0]
            # Halftone dots average to a moderate tone. Substantial continuous
            # dark artwork inside the contour does not pass as blank backing.
            if np.median(values) < 115 or np.mean(smooth < 75) > .02:
                continue
            padded = np.pad(mask, 3)
            ring = cv2.dilate(padded, np.ones((7, 7), np.uint8))-padded
            ring_values = gray[y-3:y+h+3, x-3:x+w+3][ring > 0]
            median = float(np.median(values))
            if (np.mean(ring_values < median-45) < .40
                    or median-float(np.percentile(ring_values, 30)) < 60):
                continue
            candidate.update(members=members, fill_color=[round(median)]*3,
                             solidity=solidity, score=.88, textured=np.mean(values < 210) > .15)
            if not any(_mask_overlap(candidate, other) > 0 for other in existing):
                candidates.append(candidate)
    accepted = []
    for candidate in sorted(candidates, key=lambda item: item["area"]):
        if (assigned.intersection(candidate["members"])
                or any(_mask_overlap(candidate, other) > 0 for other in accepted)):
            continue
        accepted.append(candidate)
        assigned.update(candidate["members"])
    return accepted


def _narration_groups(gray, blocks, boxes, bubbles):
    """Group aligned, unboxed vertical narration into one horizontal canvas."""
    import cv2
    import numpy as np
    height, width = gray.shape
    assigned = {index for candidate in bubbles for index in candidate["members"]}
    pending = [i for i, box in enumerate(boxes) if box and i not in assigned
               and blocks[i].get("vertical") and box[3]-box[1] >= height*.12]
    groups = []
    while pending:
        group = [pending.pop(0)]
        changed = True
        while changed:
            changed = False
            for i in pending[:]:
                a = boxes[i]
                for j in group:
                    b = boxes[j]
                    overlap = min(a[3], b[3])-max(a[1], b[1])
                    gap = max(a[0], b[0])-min(a[2], b[2])
                    if (abs(a[1]-b[1]) <= min(a[3]-a[1], b[3]-b[1])*.18
                            and overlap >= .65*min(a[3]-a[1], b[3]-b[1])
                            and -5 <= gap <= width*.07):
                        group.append(i)
                        pending.remove(i)
                        changed = True
                        break
        if len(group) < 3:
            continue
        left = max(0, min(boxes[i][0] for i in group)-10)
        top = max(0, min(boxes[i][1] for i in group)-10)
        right = min(width, max(boxes[i][2] for i in group)+10)
        bottom = min(height, max(boxes[i][3] for i in group)+10)
        if right-left < width*.35:
            continue
        candidate = {"origin": (left, top), "mask": np.ones((bottom-top, right-left), np.uint8),
                     "area": (right-left)*(bottom-top), "members": group,
                     "points": [[left, top], [right-1, top], [right-1, bottom-1], [left, bottom-1]],
                     "text_box": [left, top, right, bottom], "textured": True}
        # A nearby oval can protrude into the union's empty corner. Carve its
        # silhouette out instead of discarding all narration or painting it.
        for other in bubbles:
            x, y = other["origin"]
            mask = other["mask"]
            a, b = max(left, x-5), max(top, y-5)
            c, d = min(right, x+mask.shape[1]+5), min(bottom, y+mask.shape[0]+5)
            if c <= a or d <= b:
                continue
            exclusion = cv2.dilate(np.pad(mask, 5), np.ones((11, 11), np.uint8))
            candidate["mask"][b-top:d-top, a-left:c-left] &= 1-exclusion[b-y+5:d-y+5, a-x+5:c-x+5]
        carved = _shape(candidate["mask"], candidate["origin"])
        if carved is None or any(_coverage(carved, boxes[i]) < .94 for i in group):
            continue
        candidate.update(mask=carved["mask"], points=carved["points"], area=carved["area"])
        background = candidate["mask"].copy()
        for i in group:
            a, b, c, d = boxes[i]
            background[max(0, b-top-8):min(bottom-top, d-top+8),
                       max(0, a-left-8):min(right-left, c-left+8)] = 0
        patch = gray[top:bottom, left:right]
        if background.sum() < 100:
            continue
        low_frequency = cv2.GaussianBlur(patch, (9, 9), 2)
        if np.mean(low_frequency[background > 0] < 65) > .06:
            continue
        # A panel divider spans the narration band, whereas screentone dots
        # and individual glyph strokes do not form a continuous dark column.
        if np.any(np.mean(patch < 90, axis=0) > .82):
            continue
        # Reject a proposed combined canvas if any other OCR unit lies in it.
        if any(_coverage(candidate, box) > .02 for i, box in enumerate(boxes) if i not in group and box):
            continue
        groups.append(candidate)
    return groups


def reading_regions(page, gray):
    """Return final display regions, without changing page or image data."""
    import cv2
    import numpy as np

    if not isinstance(page, dict) or getattr(gray, "ndim", 0) != 2 or not gray.size:
        raise ValueError("Invalid page or image")
    height, width = gray.shape
    if page.get("width") != width or page.get("height") != height:
        raise ValueError("Image size does not match cached OCR coordinates")
    blocks = page.get("blocks", [])
    if not isinstance(blocks, list) or any(not isinstance(block, dict) for block in blocks):
        raise ValueError("Invalid OCR blocks")
    ids = [str(block.get("id")) for block in blocks]
    if len(set(ids)) != len(ids) or any(block.get("id") is None for block in blocks):
        raise ValueError("Missing or duplicate OCR block identifiers")
    boxes = [_box(block, width, height) for block in blocks]
    _, labels, stats, _ = cv2.connectedComponentsWithStats(cv2.inRange(gray, 210, 255), 8)
    chosen = set()
    for box in boxes:
        if box is None:
            continue
        x1, y1, x2, y2 = box
        counts = np.bincount(labels[y1:y2, x1:x2].ravel())
        counts[0] = 0
        label = int(counts.argmax())
        if label and counts[label] >= (x2-x1)*(y2-y1)*.40:
            chosen.add(label)
    bubbles = []
    for label in sorted(chosen):
        candidate = _bounded_bubble(gray, labels, stats, label, boxes)
        if candidate:
            bubbles.extend(_split_lobes(candidate, boxes))
    bubbles.extend(_outlined_bubbles(gray, boxes, bubbles))
    narrations = _narration_groups(gray, blocks, boxes, bubbles)

    regions, accepted, claimed = [], [], set()
    for candidate in sorted(bubbles, key=lambda item: min(item["members"])):
        indexes = candidate["members"]
        # Claim all members even if one is not translated.  An old singleton
        # fallback must not partially erase a newly discovered shared bubble.
        claimed.update(ids[index] for index in indexes)
        if not all(_ready(blocks[index]) for index in indexes):
            continue
        inner = _inner_rectangle(candidate["mask"])
        if inner is None:
            continue
        x, y = candidate["origin"]
        regions.append({"id": f"reading-{len(regions)+1}",
                        "block_ids": [ids[index] for index in indexes],
                        "overlay": "balloon", "overlay_score": candidate["score"],
                        "overlay_reason": "阅读显示：已确认封闭气泡，仅清除原文字范围",
                        "points": candidate["points"],
                        "text_box": [x+inner[0], y+inner[1], x+inner[2], y+inner[3]],
                        "erase_points": _erase_polygons(candidate, [boxes[index] for index in indexes]),
                        "fill_color": candidate["fill_color"],
                        "vertical": all(bool(blocks[index].get("vertical")) for index in indexes)})
        regions[-1].update(background_patch(gray, [boxes[index] for index in indexes],
                            clip=(candidate["mask"], candidate["origin"]),
                            textured=candidate.get("textured", False)))
        accepted.append(candidate)

    for candidate in narrations:
        indexes = sorted(candidate["members"], key=lambda i: (blocks[i].get("reading_order", 10**9), -boxes[i][0]))
        claimed.update(ids[index] for index in indexes)
        if not all(_ready(blocks[index]) for index in indexes):
            continue
        region = {"id": f"reading-{len(regions)+1}", "block_ids": [ids[i] for i in indexes],
                  "overlay": "narration", "overlay_score": .85,
                  "overlay_reason": "阅读显示：连续旁白横排，保留背景纹理",
                  "points": candidate["points"], "text_box": candidate["text_box"],
                  "erase_points": [], "fill_color": [255, 255, 255], "vertical": False}
        region.update(background_patch(gray, [boxes[i] for i in indexes],
                      clip=(candidate["mask"], candidate["origin"]), textured=True))
        regions.append(region)
        accepted.append(candidate)

    by_id = dict(zip(ids, blocks))
    for old in page.get("overlay_regions", []):
        if not isinstance(old, dict) or old.get("overlay") not in {"balloon", "rectangle"}:
            continue
        members = old.get("block_ids", [])
        if not isinstance(members, list) or not members:
            continue
        members = [str(value) for value in members]
        if (claimed.intersection(members) or any(key not in by_id or not _ready(by_id[key]) for key in members)):
            continue
        shape = _cached_shape(old, width, height)
        if shape is None or any(_mask_overlap(shape, other) > 0 for other in bubbles+narrations+accepted):
            continue
        member_boxes = [boxes[ids.index(key)] for key in members]
        if any(box is None for box in member_boxes):
            continue
        region = copy.deepcopy(old)
        x, y = shape["origin"]
        patch = gray[y:y+shape["mask"].shape[0], x:x+shape["mask"].shape[1]]
        textured = bool(np.mean(patch < 210) > .18)
        region.update(background_patch(gray, member_boxes,
                      clip=(shape["mask"], shape["origin"]) if old["overlay"] == "balloon" else None,
                      textured=textured))
        regions.append(region)
        claimed.update(members)
        accepted.append(shape)
    return regions


def main(argv=None):
    import cv2
    import numpy as np

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True)
    parser.add_argument("--cache", required=True)
    args = parser.parse_args(argv)
    page = json.loads(Path(args.cache).read_text(encoding="utf-8"))
    gray = cv2.imdecode(np.frombuffer(Path(args.image).read_bytes(), np.uint8), cv2.IMREAD_GRAYSCALE)
    regions = reading_regions(page, gray)
    print(json.dumps({"version": VERSION, "regions": regions}, ensure_ascii=False, separators=(",", ":")))


if __name__ == "__main__":
    main()

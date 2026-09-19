"""Conservative text-bearing panel candidates from full-span image gutters.

Candidates guide grouping/order only, never authorize erasure. Unsplit or
nonrectangular panels retain the geometric reading-order fallback.
"""
from __future__ import annotations
import hashlib
import json
from pathlib import Path

VERSION = 1


def _valid_box(block):
    import math
    try:
        box = list(map(float, block.get('box', [])))
        if len(box) == 4 and all(map(math.isfinite, box)) and box[2] > box[0] and box[3] > box[1]:
            return box
    except (TypeError, ValueError):
        pass
    return None


def _runs(mask):
    import numpy as np
    edges = np.flatnonzero(np.diff(np.r_[False, mask, False].astype(np.int8)))
    return list(zip(edges[::2], edges[1::2]))


def detect_panels(gray, blocks):
    import numpy as np
    height, width = gray.shape
    boxes = {str(b['id']): _valid_box(b) for b in blocks if b.get('id') is not None}
    boxes = {bid: box for bid, box in boxes.items() if box is not None
             and 0 <= box[0] < box[2] <= width and 0 <= box[1] < box[3] <= height}
    leaves, cuts = [], []
    minimum = (max(3, round(width * .004)), max(3, round(height * .004)))

    def recurse(bounds, ids, depth=0):
        left, top, right, bottom = bounds
        if depth >= 12 or len(ids) < 2:
            leaves.append((bounds, ids)); return
        roi = gray[top:bottom, left:right]
        # Full-span gutters can be black or white. Two sides must each contain
        # complete text boxes, so borders and empty margins never form panels.
        for axis in (1, 0):
            reduction = 1 if axis == 1 else 0
            blank = np.maximum((roi <= 24).mean(axis=reduction), (roi >= 245).mean(axis=reduction)) >= .985
            candidates = []
            for lo, hi in _runs(blank):
                if hi-lo < minimum[axis]:
                    continue
                start, end = int(lo)+(top if axis == 1 else left), int(hi)+(top if axis == 1 else left)
                before = [bid for bid in ids if boxes[bid][axis+2] <= start]
                after = [bid for bid in ids if boxes[bid][axis] >= end]
                if not before or not after or len(before)+len(after) != len(ids):
                    continue
                extent = height if axis == 1 else width
                if start-bounds[axis] < extent * .09 or bounds[axis+2]-end < extent * .09:
                    continue
                candidates.append((hi-lo, start, end, before, after))
            if candidates:
                # Prefer a wide gutter. Recursion resolves remaining gutters;
                # fixed tie-breaks keep results independent of detector order.
                _, start, end, before, after = max(candidates, key=lambda x: (x[0], -x[1]))
                mid = (start+end)//2
                a, b = list(bounds), list(bounds)
                a[axis+2], b[axis] = mid, mid
                cuts.append({'axis': 'horizontal' if axis == 1 else 'vertical',
                             'start': start, 'end': end, 'bounds': list(bounds)})
                ordered = [(a, before), (b, after)] if axis == 1 else [(b, after), (a, before)]
                for box, children in ordered:
                    recurse(box, children, depth+1)
                return
        leaves.append((bounds, ids))

    if boxes:
        recurse([0, 0, width, height], sorted(boxes))
    return [{'id': f'p{i}', 'box': list(bounds), 'block_ids': ids, 'reading_order': i,
             'method': 'image_gutters' if cuts else 'page_fallback', 'needs_review': True}
            for i, (bounds, ids) in enumerate(leaves, 1)], cuts


def ensure_panels(page, gray=None, image=None):
    inputs = sorted([[str(b.get('id')), b.get('box')] for b in page.get('blocks', [])], key=lambda x: x[0])
    source_signature = hashlib.sha256(json.dumps(inputs, sort_keys=True).encode()).hexdigest()
    old = page.get('panel_signature', [])

    def fallback(reason):
        changed = 'panels' in page or page.get('panel_error') != reason
        page.pop('panels', None)
        page.pop('panel_signature', None)
        page.pop('panel_cuts', None)
        page['panel_error'] = reason
        return changed

    if gray is None and image is None:
        # Reading an image-backed cache requires no image runtime. Reuse only
        # while its geometry and algorithm version still match.
        if len(old) == 3 and old[0] == VERSION and old[1] == source_signature and 'panels' in page:
            return False
        return fallback('缺少图像且 OCR 几何已变化，回退到文字框顺序')
    try:
        import numpy as np
    except (ImportError, OSError) as exc:
        return fallback(f'NumPy 无法加载 ({type(exc).__name__}: {exc})，回退到文字框顺序')
    if gray is None and image is not None:
        try:
            import cv2
        except (ImportError, OSError, AttributeError) as exc:
            return fallback(f'OpenCV 无法加载 ({type(exc).__name__}: {exc})，回退到文字框顺序')
        try:
            gray = cv2.imdecode(np.frombuffer(Path(image).read_bytes(), np.uint8), cv2.IMREAD_GRAYSCALE)
        except (OSError, ValueError, cv2.error):
            gray = None
    if gray is not None and (getattr(gray, 'ndim', 0) != 2 or not gray.size):
        gray = None
    if gray is None:
        return fallback('无法读取图像，回退到文字框顺序')
    signature = [VERSION, source_signature, hashlib.sha256(gray.tobytes()).hexdigest()]
    if page.get('panel_signature') == signature and 'panels' in page:
        return False
    panels, cuts = detect_panels(gray, page.get('blocks', []))
    page.pop('panel_error', None)
    page.update(panels=panels, panel_cuts=cuts, panel_signature=signature)
    return True

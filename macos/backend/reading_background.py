"""Local source-ink restoration for display; never writes source images."""
from __future__ import annotations

import base64


def _texture_fill(patch, ink):
    """Restore the smooth tone, then borrow aligned nearby screentone detail."""
    import cv2
    import numpy as np

    smooth = cv2.GaussianBlur(patch, (0, 0), 2.2)
    tone_mask = cv2.dilate(ink, np.ones((13, 13), np.uint8))
    restored = cv2.inpaint(smooth, tone_mask, 7, cv2.INPAINT_NS).astype(np.float32)
    detail = patch.astype(np.float32)-smooth.astype(np.float32)
    clean = tone_mask == 0
    # Donors can extend beyond the current balloon. Exclude continuous ink
    # there too, so a nearby outline cannot reappear as a dark text residue.
    count, labels, stats, _ = cv2.connectedComponentsWithStats((patch < 85).astype(np.uint8), 8)
    forbidden = np.zeros(count, np.uint8)
    for index in range(1, count):
        _, _, w, h, area = map(int, stats[index])
        if area >= 10 and max(w, h) >= 7:
            forbidden[index] = 1
    clean &= cv2.dilate(forbidden[labels], np.ones((5, 5), np.uint8)) == 0
    # Screentones are periodic. Rank offsets using only known clean pixels;
    # a matching phase avoids replacing dots with a flat white/gray card.
    shifts = []
    for dy in (-48, -32, -16, 0, 16, 32, 48):
        for dx in range(-80, 81, 4):
            if abs(dx)+abs(dy) < 12:
                continue
            shifted = np.roll(detail, (dy, dx), (0, 1))
            valid = clean & np.roll(clean, (dy, dx), (0, 1))
            valid[:max(0, dy), :] = False
            if dy < 0:
                valid[dy:, :] = False
            valid[:, :max(0, dx)] = False
            if dx < 0:
                valid[:, dx:] = False
            if valid.sum() < 300:
                continue
            error = float(np.mean((detail[::2, ::2][valid[::2, ::2]]-shifted[::2, ::2][valid[::2, ::2]])**2))
            shifts.append((error, dx, dy))
    best = np.full(patch.shape, np.inf, np.float32)
    result = restored.copy()
    finalists = []
    for low, high in ((0, 24), (24, 48), (48, 128)):
        finalists.extend(sorted(item for item in shifts if low < abs(item[1])+abs(item[2]) <= high)[:8])
    for phase_error, dx, dy in finalists:
        valid = np.roll(clean, (dy, dx), (0, 1)) & (ink > 0)
        valid[:max(0, dy), :] = False
        if dy < 0:
            valid[dy:, :] = False
        valid[:, :max(0, dx)] = False
        if dx < 0:
            valid[:, dx:] = False
        tone = np.roll(smooth, (dy, dx), (0, 1)).astype(np.float32)
        cost = np.abs(tone-restored)+.025*(abs(dx)+abs(dy))+.002*phase_error
        use = valid & (cost < best)
        result[use] = restored[use]+np.roll(detail, (dy, dx), (0, 1))[use]
        best[use] = cost[use]
    return np.clip(result, 0, 255).astype(np.uint8)


def background_patch(gray, boxes, *, clip=None, textured=False):
    """Encode a crop whose changed pixels are limited to local source ink.

    clip is (mask, (x,y)) for a confirmed balloon.  The outline is protected
    before source connected components are selected, including small glyph
    residues just beyond the OCR box. Textured crops retain unmasked pixels
    byte-for-byte and reconstruct only glyphs plus their white outline.
    """
    import cv2
    import numpy as np

    reach = min(24, max(10, int(min(min(b[2]-b[0], b[3]-b[1]) for b in boxes)*.28)))
    margin = max(reach+8, 68 if textured else 0)
    left = max(0, min(b[0] for b in boxes)-margin)
    top = max(0, min(b[1] for b in boxes)-margin)
    right = min(gray.shape[1], max(b[2] for b in boxes)+margin)
    bottom = min(gray.shape[0], max(b[3] for b in boxes)+margin)
    patch = gray[top:bottom, left:right].copy()
    allowed = np.ones_like(patch, np.uint8)
    if clip is not None:
        mask, (x, y) = clip
        safe = cv2.erode(np.pad(mask, 3), np.ones((7, 7), np.uint8))[3:-3, 3:-3]
        allowed[:] = 0
        a, b, c, d = max(left, x), max(top, y), min(right, x+mask.shape[1]), min(bottom, y+mask.shape[0])
        if c > a and d > b:
            allowed[b-top:d-top, a-left:c-left] = safe[b-y:d-y, a-x:c-x]
    seeds = np.zeros_like(patch, np.uint8)
    for x1, y1, x2, y2 in boxes:
        seeds[max(0, y1-top-reach):min(patch.shape[0], y2-top+reach),
              max(0, x1-left-reach):min(patch.shape[1], x2-left+reach)] = 1
    dark = ((patch < (75 if textured else 205)) & (allowed > 0)).astype(np.uint8)
    count, labels, stats, _ = cv2.connectedComponentsWithStats(dark, 8)
    overlaps = np.bincount(labels[seeds > 0].ravel(), minlength=count)
    selected = np.zeros(count, np.uint8)
    for index in range(1, count):
        x, y, w, h, area = map(int, stats[index])
        # Long scene lines cannot become a source glyph merely by touching
        # an OCR box. Small detached punctuation remains eligible.
        glyph = area >= 10 and max(w, h) >= 6
        if textured and not glyph and area >= 3:
            surround = patch[max(0, y-3):y+h+3, max(0, x-3):x+w+3]
            glyph = float(np.mean(surround)) > (190 if area >= 7 else 220)
        if overlaps[index] >= 2 and (glyph if textured else area >= 2) and max(w, h) <= max(90, reach*5):
            selected[index] = 1
    ink = selected[labels]
    if textured:
        # Remove the white counters/outline inside each selected glyph too;
        # retaining those holes leaves a recognizable pale Japanese ghost.
        for index in np.flatnonzero(selected):
            x, y, w, h, _ = map(int, stats[index])
            ink[y:y+h, x:x+w] = 1
    # A white outline belongs to the source glyph too. Closing the small
    # gaps between its strokes lets the restoration remove that halo.
    if textured:
        ink = cv2.morphologyEx(ink, cv2.MORPH_CLOSE, np.ones((5, 5), np.uint8))
    radius = 6 if textured else 2
    ink = cv2.dilate(ink, cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (2*radius+1, 2*radius+1)))
    ink &= allowed
    byte_mask = ink*255
    if not ink.any():
        restored = patch.copy()
    else:
        restored = _texture_fill(patch, byte_mask) if textured else cv2.inpaint(patch, byte_mask, 3, cv2.INPAINT_TELEA)
    # A rectangular PNG is just a transport container, never a rectangle fill.
    restored[ink == 0] = patch[ink == 0]
    rgba = cv2.cvtColor(restored, cv2.COLOR_GRAY2BGRA)
    rgba[:, :, 3] = byte_mask
    ok, png = cv2.imencode(".png", rgba)
    if not ok:
        raise ValueError("Could not encode reading background patch")
    return {"background_png": base64.b64encode(png).decode("ascii"),
            "background_box": [left, top, right, bottom]}

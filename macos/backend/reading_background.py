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
    # Judge boundaries at the tone scale. Dark dot/stripe components often
    # connect across a whole screentone and must not all be excluded as ink.
    # Known source glyphs already have a padded exclusion above.
    gradient = cv2.morphologyEx(smooth, cv2.MORPH_GRADIENT, np.ones((7, 7), np.uint8))
    clean &= gradient < 55
    # Screentones are periodic. Rank offsets using only known clean pixels;
    # a matching phase avoids replacing dots with a flat white/gray card.
    shifts = []
    for dy in (-96, -64, -32, -16, 0, 16, 32, 64, 96):
        for dx in range(-160, 161, 4):
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
    # Global phase winners alone can all point into another source column.
    # Evaluate the local validity of every offset so dense text still has
    # usable donors on both sides of the band.
    for phase_error, dx, dy in sorted(shifts):
        valid = np.roll(clean, (dy, dx), (0, 1)) & (ink > 0)
        valid[:max(0, dy), :] = False
        if dy < 0:
            valid[dy:, :] = False
        valid[:, :max(0, dx)] = False
        if dx < 0:
            valid[:, dx:] = False
        tone = np.roll(smooth, (dy, dx), (0, 1)).astype(np.float32)
        valid &= np.abs(tone-restored) < 22
        cost = np.abs(tone-restored)+.025*(abs(dx)+abs(dy))+.002*phase_error
        use = valid & (cost < best)
        # Copy actual nearby texture. Adding detail to an inpainted mean can
        # turn black/white printed dots into a visibly flat gray glyph ghost.
        result[use] = np.roll(patch, (dy, dx), (0, 1))[use]
        best[use] = cost[use]
    return np.clip(result, 0, 255).astype(np.uint8)


def background_patch(gray, boxes, *, clip=None, textured=False, color=None, source_mask=None, restorer=None):
    """Encode a crop whose changed pixels are limited to local source ink.

    clip is (mask, (x,y)) for a confirmed balloon.  The outline is protected
    before source connected components are selected, including small glyph
    residues just beyond the OCR box. Textured crops retain unmasked pixels
    byte-for-byte and reconstruct only glyphs plus their white outline.
    """
    import cv2
    import numpy as np

    reach = min(24, max(10, int(min(min(b[2]-b[0], b[3]-b[1]) for b in boxes)*.28)))
    margin = max(reach+8, 96)
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
    # Label before clipping: clipping first can turn a long hair/outline
    # crossing the text footprint into a short, apparently glyph-like stroke.
    dark = (patch < (75 if textured else 205)).astype(np.uint8)
    count, labels, stats, _ = cv2.connectedComponentsWithStats(dark, 8)
    overlaps = np.bincount(labels[(seeds > 0) & (allowed > 0)].ravel(), minlength=count)
    contained = np.bincount(labels[allowed > 0].ravel(), minlength=count)
    selected = np.zeros(count, np.uint8)
    dashes = np.zeros(count, np.uint8)
    for index in range(1, count):
        x, y, w, h, area = map(int, stats[index])
        if contained[index] < area*.98:
            continue
        # Long scene lines cannot become a source glyph merely by touching
        # an OCR box. Small detached punctuation remains eligible.
        glyph = area >= 10 and max(w, h) >= 6
        if textured and glyph:
            surround = patch[max(0, y-3):y+h+3, max(0, x-3):x+w+3]
            border = np.ones(surround.shape, bool)
            by, bx = y-max(0, y-3), x-max(0, x-3)
            border[by:by+h, bx:bx+w] = False
            glyph = bool(border.any() and np.mean(surround[border]) > 145)
        if textured and not glyph and area >= 3:
            surround = patch[max(0, y-3):y+h+3, max(0, x-3):x+w+3]
            glyph = float(np.mean(surround)) > (190 if area >= 7 else 220)
        # Long vowel/dash strokes belong to the dialogue when almost fully
        # contained in its OCR band. Keep scene lines that merely cross it.
        aligned = any(a-4 <= left+x+w/2 <= c+4 and b-80 <= top+y and top+y+h <= d+80
                      for a, b, c, d in boxes)
        dash = (not textured and min(w, h) <= 8 and max(w, h) >= 30
                and max(w, h) >= 8*min(w, h) and (overlaps[index] >= area*.98 or (clip is not None and h > w and aligned)))
        if dash:
            dashes[index] = 1
        if overlaps[index] >= 2 and (glyph if textured else area >= 2) and (max(w, h) <= max(90, reach*5) or dash):
            selected[index] = 1
    ink = selected[labels]
    if textured:
        # Remove the white counters/outline inside each selected glyph too;
        # retaining those holes leaves a recognizable pale Japanese ghost.
        for index in np.flatnonzero(selected):
            x, y, w, h, _ = map(int, stats[index])
            ink[y:y+h, x:x+w] = 1
        # White lettering/halos on a dark ground cannot be found by a dark
        # connected-component detector: their black counters join the ground.
        core = np.zeros_like(seeds)
        for x1, y1, x2, y2 in boxes:
            core[max(0, y1-top):y2-top, max(0, x1-left):x2-left] = 1
        count, light_labels, light_stats, _ = cv2.connectedComponentsWithStats(
            (patch > 210).astype(np.uint8), 8)
        inside = np.bincount(light_labels[core > 0].ravel(), minlength=count)
        light_contained = np.bincount(light_labels[allowed > 0].ravel(), minlength=count)
        dark_source_ground = bool(core.any() and np.median(patch[core > 0]) < 180)
        for index in range(1, count):
            x, y, w, h, area = map(int, light_stats[index])
            if (area < 8 or max(w, h) < 6 or max(w, h) > max(90, reach*5)
                    or inside[index] < area*.75 or light_contained[index] < area*.98 or not dark_source_ground):
                continue
            around = patch[max(0, y-5):y+h+5, max(0, x-5):x+w+5]
            if np.median(around) < 185:
                ink[y:y+h, x:x+w] = 1
    # A white outline belongs to the source glyph too. Closing the small
    # gaps between its strokes lets the restoration remove that halo.
    if textured:
        ink = cv2.morphologyEx(ink, cv2.MORPH_CLOSE, np.ones((5, 5), np.uint8))
    if source_mask is not None:
        # Use detector evidence rather than brightness guesses when available.
        # Selection is still limited to this region's translated source band.
        ink = ((source_mask[top:bottom, left:right] > 0) & (seeds > 0)).astype(np.uint8)
        # The detector can omit a long standalone Japanese dash. Recover
        # only narrow, fully contained strokes in a confirmed plain balloon.
        ink |= dashes[labels]
        if ink.any():
            distance = cv2.distanceTransform(1-ink, cv2.DIST_L2, 5)
            # Recover isolated small strokes next to detected lettering on
            # a white ground (e.g. the three strokes of ミ). Large artwork,
            # crossing scene lines and dark screentones are not eligible.
            for index in np.flatnonzero(selected):
                x, y, w, h, area = map(int, stats[index])
                if not (8 <= area <= 900 and 3 <= min(w, h) and max(w, h) <= 60
                        and overlaps[index] >= area*.98 and contained[index] >= area*.98):
                    continue
                component = labels[y:y+h, x:x+w] == index
                if distance[y:y+h, x:x+w][component].min() > 40:
                    continue
                around = patch[max(0, y-4):y+h+4, max(0, x-4):x+w+4]
                border = np.ones(around.shape, bool)
                by, bx = y-max(0, y-4), x-max(0, x-4)
                border[by:by+h, bx:bx+w] = False
                # Nearby screentone or another stroke can lower the mean
                # even when the component is clearly isolated on white.
                # Require predominantly white surroundings as well as a
                # bright mean, instead of rejecting the whole glyph at 235.
                if (border.any() and np.mean(around[border]) > 220
                        and np.mean(around[border] >= 245) >= .65):
                    ink[y:y+h, x:x+w][component] = 1
        surroundings = (cv2.dilate(ink, np.ones((17, 17), np.uint8)) > 0) & (ink == 0) & (allowed > 0)
        textured = textured or bool(surroundings.any() and (
            np.mean(patch[surroundings] < 235) > .12 or np.std(patch[surroundings]) > 8))
    radius = (5 if source_mask is not None else 6) if textured else 2
    ink = cv2.dilate(ink, cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (2*radius+1, 2*radius+1)))
    ink &= allowed
    if source_mask is None:
        # Padding around a legitimate glyph must not eat into a rejected
        # neighboring component (hair, an illustration, or the balloon rim).
        ink[(dark > 0) & (selected[labels] == 0)] = 0
    byte_mask = ink*255
    restored_color = None
    if ink.any() and textured and restorer is not None:
        crop = patch if color is None else color[top:bottom, left:right]
        reconstructed = restorer(crop, byte_mask)
        if color is None:
            restored = reconstructed
        else:
            restored_color = reconstructed
            restored = cv2.cvtColor(reconstructed, cv2.COLOR_BGR2GRAY)
    elif not ink.any():
        restored = patch.copy()
    else:
        restored = _texture_fill(patch, byte_mask) if textured else cv2.inpaint(patch, byte_mask, 3, cv2.INPAINT_TELEA)
    # A rectangular PNG is just a transport container, never a rectangle fill.
    restored[ink == 0] = patch[ink == 0]
    if color is not None:
        crop = color[top:bottom, left:right]
        if restored_color is not None:
            rgba = cv2.cvtColor(restored_color, cv2.COLOR_BGR2BGRA)
        else:
            channels = cv2.cvtColor(crop, cv2.COLOR_BGR2YCrCb)
            channels[:, :, 0] = restored
            if ink.any():
                for index in (1, 2):
                    channels[:, :, index] = cv2.inpaint(channels[:, :, index], byte_mask, 3, cv2.INPAINT_TELEA)
            rgba = cv2.cvtColor(cv2.cvtColor(channels, cv2.COLOR_YCrCb2BGR), cv2.COLOR_BGR2BGRA)
        rgba[ink == 0, :3] = crop[ink == 0]
    else:
        rgba = cv2.cvtColor(restored, cv2.COLOR_GRAY2BGRA)
    rgba[:, :, 3] = byte_mask
    ok, png = cv2.imencode(".png", rgba)
    if not ok:
        raise ValueError("Could not encode reading background patch")
    return {"background_png": base64.b64encode(png).decode("ascii"),
            # Original white glyph halos can dominate a small dark balloon.
            # Choose foreground contrast against the cleaned ground instead.
            "dark_background": bool(allowed.any() and np.median(restored[allowed > 0]) < 115),
            "background_box": [left, top, right, bottom]}

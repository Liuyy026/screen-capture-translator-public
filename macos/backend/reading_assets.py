"""Disposable display assets, separate from original images and OCR caches."""
from __future__ import annotations

import contextlib
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile


def atomic_write(path, data):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=path.name + ".", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def asset_key(image_bytes, cache_bytes, model_paths=()):
    digest = hashlib.sha256(image_bytes + b"\0" + cache_bytes)
    # Code changes invalidate only disposable rendering assets, never translation.
    for name in ("reading_assets.py", "reading_overlay.py", "reading_background.py"):
        digest.update(Path(__file__).with_name(name).read_bytes())
    for path in model_paths:
        path = Path(path)
        digest.update(str(path.resolve()).encode())
        if path.exists():
            stat = path.stat()
            digest.update(f"{stat.st_size}:{stat.st_mtime_ns}".encode())
        else:
            digest.update(b"missing")
    return digest.hexdigest()


def text_mask(image, image_bytes, weights, directory):
    """Reuse the installed detector; no recognition or translation request."""
    import cv2
    import numpy as np

    weights = Path(weights)
    stat = weights.stat()
    key = hashlib.sha256(image_bytes + f"ctd-mask-v2:{stat.st_size}:{stat.st_mtime_ns}".encode()).hexdigest()
    target = Path(directory) / "masks" / (key + ".png")
    if target.exists():
        mask = cv2.imdecode(np.frombuffer(target.read_bytes(), np.uint8), cv2.IMREAD_GRAYSCALE)
        if mask is not None and mask.shape == image.shape[:2]:
            return mask
    import torch
    torch.set_num_threads(4)
    # Detector progress belongs on stderr; stdout is the reader's JSON protocol.
    with contextlib.redirect_stdout(sys.stderr):
        from comic_text_detector.inference import TextDetector
        detector = TextDetector(model_path=str(weights), input_size=1024, device="cpu", act="leaky")
        raw, mask, _ = detector(image, refine_mode=0, keep_undetected_mask=True)
    # Refinement can drop detached katakana strokes and punctuation. Retain
    # high-confidence neural pixels as well; final use remains clipped to
    # translated source bands and confirmed display contours.
    mask = ((mask > 0) | (raw > 160)).astype(np.uint8) * 255
    ok, data = cv2.imencode(".png", mask)
    if not ok:
        raise ValueError("Cannot encode text segmentation")
    atomic_write(target, data.tobytes())
    return mask


class LocalRestorer:
    """Lazy, CPU-only Anime LaMa inference on bounded local crops.

    Weights are provisioned explicitly, never downloaded during page opening.
    Only the supplied mask is composited back by background_patch.
    """
    def __init__(self, weights):
        self.weights = Path(weights)
        self.model = None

    def __call__(self, image, mask):
        import cv2
        import numpy as np
        import torch

        if self.model is None:
            torch.set_num_threads(4)
            self.model = torch.jit.load(str(self.weights), map_location="cpu").eval()
        height, width = mask.shape
        rgb = cv2.cvtColor(image, cv2.COLOR_GRAY2RGB if image.ndim == 2 else cv2.COLOR_BGR2RGB)
        factor = min(1.0, 1024 / max(height, width))
        if factor < 1:
            size = (max(1, round(width * factor)), max(1, round(height * factor)))
            rgb = cv2.resize(rgb, size, interpolation=cv2.INTER_AREA)
            mask = cv2.resize(mask, size, interpolation=cv2.INTER_NEAREST)
        h, w = mask.shape
        rgb = cv2.copyMakeBorder(rgb, 0, (-h) % 8, 0, (-w) % 8, cv2.BORDER_REFLECT)
        mask = cv2.copyMakeBorder(mask, 0, (-h) % 8, 0, (-w) % 8, cv2.BORDER_CONSTANT)
        values = torch.from_numpy(rgb.transpose(2, 0, 1).copy()).unsqueeze(0).float() / 255
        selection = torch.from_numpy((mask > 0).astype(np.float32)).unsqueeze(0).unsqueeze(0)
        with torch.inference_mode():
            output = self.model(values, selection)[0].permute(1, 2, 0).cpu().numpy()
        output = np.clip(output[:h, :w] * 255, 0, 255).astype(np.uint8)
        if factor < 1:
            output = cv2.resize(output, (width, height), interpolation=cv2.INTER_CUBIC)
        return cv2.cvtColor(output, cv2.COLOR_RGB2GRAY if image.ndim == 2 else cv2.COLOR_RGB2BGR)

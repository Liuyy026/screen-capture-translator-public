#!/usr/bin/env python3
"""Offline GPU regression for SE synchronization; private pages stay local.

Requires Pillow and numpy for verification only. Use --helper and --assets to
test an isolated build without replacing the application's installed helper.
The output directory must be new, so an earlier review is never overwritten.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

import numpy as np
from PIL import Image, ImageDraw, ImageOps


def main():
    root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--helper", type=Path, default=root / "macos/.build/koma-sr-realcugan")
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--page", type=Path, help="Optional local monochrome regression page; no scan or upload")
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    env = os.environ.copy()
    if args.assets:
        env["KOMA_REALCUGAN_ROOT"] = str(args.assets.resolve())
    helper = str(args.helper.resolve())
    caps = subprocess.run([helper, "--capabilities"], env=env, check=True, capture_output=True, text=True)
    capabilities = json.loads(caps.stdout)
    assert capabilities["available"], capabilities
    records = []

    def run(source, name, size, preset, monochrome=False, reject_cast=False):
        destination = out / (name + ".png")
        started = time.monotonic()
        result = subprocess.run(
            [helper, "--input", str(source), "--output", str(destination),
             "--width", str(size[0]), "--height", str(size[1]), "--preset", preset],
            env=env, capture_output=True, text=True, timeout=120,
        )
        (out / (name + ".stderr")).write_text(result.stderr)
        assert result.returncode == 0, (name, result.stderr)
        metadata = json.loads(result.stdout)
        image = Image.open(destination).convert("RGBA")
        assert image.size == size, (name, image.size, size)
        assert metadata["backend_version"] == capabilities["backend_version"]
        record = {"name": name, "metadata": metadata, "wall_ms": 1000 * (time.monotonic() - started)}
        if monochrome:
            rgb = np.asarray(image)[:, :, :3].astype(np.int16)
            chroma = rgb.max(axis=2) - rgb.min(axis=2)
            record["mean_channel_spread"] = float(chroma.mean())
            record["fraction_spread_over_80"] = float((chroma > 80).mean())
            if reject_cast:
                # Bounds for the optional real monochrome page detect the
                # observed page-wide cast, not artistic quality. Synthetic
                # high-frequency patterns can elicit colored model fringes;
                # retain their measurements and check their white area below.
                assert record["mean_channel_spread"] < 12, record
                assert record["fraction_spread_over_80"] < .02, record
        records.append(record)
        return image

    # Dense monochrome tones, thin edges, and partial tiles at right/bottom.
    yy, xx = np.indices((383, 257))
    mono = np.full((383, 257), 255, dtype=np.uint8)
    mono[30:353, 20:237] = np.where((xx[30:353, 20:237] + yy[30:353, 20:237]) % 4 == 0, 40, 210)
    synthetic = Image.fromarray(mono).convert("RGB")
    drawing = ImageDraw.Draw(synthetic)
    drawing.ellipse((65, 60, 195, 190), fill="white", outline="black", width=2)
    drawing.text((100, 115), "KOMA", fill="black")
    drawing.line((25, 330, 225, 230), fill="black", width=2)
    source = out / "synthetic-tones.png"
    synthetic.save(source)
    for scale in (2, 3, 4):
        for preset in ("conservative", "standard", "denoise"):
            image = run(source, f"tones-{scale}x-{preset}", (257 * scale, 383 * scale), preset, monochrome=True)
            white = np.asarray(image.crop((10 * scale, 0, 200 * scale, 20 * scale)))[:, :, :3].astype(np.int16)
            assert white.min() > 240 and (white.max(2) - white.min(2)).max() < 10, (scale, preset)

    # Mode 3's sparse 32px sampling has no columns on this narrow input.
    narrow = out / "narrow.png"
    ImageOps.grayscale(synthetic).crop((0, 0, 47, 193)).save(narrow)
    for preset in ("conservative", "standard", "denoise"):
        run(narrow, "narrow-" + preset, (94, 386), preset, monochrome=True)

    # Color and alpha are preserved; the repair must never force grayscale.
    color = Image.new("RGBA", (257, 193), (255, 255, 255, 0))
    drawing = ImageDraw.Draw(color)
    drawing.rectangle((16, 16, 112, 176), fill=(255, 0, 0, 255))
    drawing.rectangle((144, 16, 240, 176), fill=(0, 0, 255, 255))
    source = out / "color-alpha.png"
    color.save(source)
    for preset in ("conservative", "standard", "denoise"):
        image = run(source, "color-" + preset, (514, 386), preset)
        red, blue = image.getpixel((128, 192)), image.getpixel((384, 192))
        assert red[0] > 200 and red[1] < 50 and red[2] < 50 and red[3] == 255, (preset, red)
        assert blue[2] > 200 and blue[0] < 50 and blue[1] < 50 and blue[3] == 255, (preset, blue)
        assert image.getpixel((256, 8))[3] == 0

    # EXIF orientation plus a small non-tiled input still obey the protocol.
    orientation = Image.new("RGB", (96, 64), "white")
    drawing = ImageDraw.Draw(orientation)
    drawing.rectangle((0, 0, 32, 24), fill="red")
    drawing.rectangle((64, 40, 95, 63), fill="blue")
    exif = Image.Exif()
    exif[274] = 6
    source = out / "orientation.jpg"
    orientation.save(source, exif=exif)
    image = run(source, "oriented", (128, 192), "conservative")
    assert image.getpixel((110, 20))[0] > 200
    assert image.getpixel((20, 175))[2] > 200

    original_hash = None
    if args.page:
        page = args.page.resolve()
        original_hash = hashlib.sha256(page.read_bytes()).hexdigest()
        source_image = ImageOps.exif_transpose(Image.open(page))
        target = tuple(round(value * 2160 / max(source_image.size)) for value in source_image.size)
        for preset in ("conservative", "standard", "denoise"):
            run(page, "private-page-" + preset, target, preset, monochrome=True, reject_cast=True)
        assert hashlib.sha256(page.read_bytes()).hexdigest() == original_hash

    assert capabilities["backend_version"] != "koma-realcugan-1", "Changed inference must invalidate old enhanced-image caches"
    assert capabilities["syncgap"] == 1
    assert not list(out.glob(".koma-realcugan-*")), "Intermediate directories must be removed"
    report = {"capabilities": capabilities, "runs": records, "private_page_sha256": original_hash}
    (out / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"PASS: {len(records)} real GPU runs; dimensions, monochrome bounds, color, alpha, orientation, source invariance")


if __name__ == "__main__":
    main()

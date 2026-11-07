#!/usr/bin/env python3
"""Render existing translations with the native reader and audit their bounds.

Run with macos/.venv/bin/python. Never recognizes or translates text, edits
page caches, or calls a translation endpoint. Display segmentation is local.
"""
from pathlib import Path
import argparse
import base64
import hashlib
import json
import subprocess
import sys
import time
import unicodedata

ROOT = Path(__file__).resolve().parents[2]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    import cv2
    import numpy as np
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--folder", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pages", help="Comma-separated one-based page indexes")
    parser.add_argument("--cache-root", type=Path, default=Path.home()/"Library/Application Support/KomaReader/pages")
    parser.add_argument("--assets-dir", type=Path, default=Path.home()/"Library/Application Support/KomaReader/reading-v3")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    renderer = args.output / "render-review"
    subprocess.run(["xcrun", "swiftc", "-O", "-o", str(renderer),
                    str(ROOT/"macos/Sources/ReadingLayout.swift"),
                    str(ROOT/"macos/Sources/ReadingOverlayView.swift"),
                    str(ROOT/"macos/tools/RenderReview.swift")], check=True)
    selected = {int(n) for n in args.pages.split(",")} if args.pages else None
    images = sorted(p for p in args.folder.iterdir() if p.suffix.lower() in {".webp", ".png", ".jpg", ".jpeg"})
    results = []
    for index, image in enumerate(images, 1):
        if selected and index not in selected:
            continue
        image_hash = sha(image)
        choices = [p for p in (args.cache_root/f"{image_hash}.json", args.cache_root/"vision-v1"/f"{image_hash}.json") if p.is_file()]
        if not choices:
            results.append(dict(page=index, image=image.name, error="No existing translation cache"))
            continue
        cache = max(choices, key=lambda p: p.stat().st_mtime_ns)
        cache_hash = sha(cache)
        page = json.loads(cache.read_bytes())
        prefix = args.output/f"{index:05}"
        start = time.monotonic()
        reply = subprocess.run([sys.executable, str(ROOT/"macos/backend/reading_overlay.py"),
                                "--image", str(image.resolve()), "--cache", str(cache),
                                "--assets-dir", str(args.assets_dir)], capture_output=True)
        if reply.returncode:
            sys.stderr.write(reply.stderr.decode("utf-8", errors="replace"))
            reply.check_returncode()
        prefix.with_name(prefix.name+"-regions.json").write_bytes(reply.stdout)
        subprocess.run([str(renderer), str(image.resolve()), str(cache),
                        str(prefix.with_name(prefix.name+"-regions.json")), str(prefix)], check=True)
        audit = json.loads(prefix.with_suffix(".json").read_bytes())
        payload = json.loads(reply.stdout)
        regions = {r["id"]: r for r in payload["regions"]}
        crossings, text_errors = [], []
        for item in audit["items"]:
            polygon = np.asarray(regions[item["id"]]["points"], np.float32)
            expected = "".join(line.strip() for line in item["text"].splitlines())
            if "".join(g["text"] for g in item["glyphs"]) != expected:
                text_errors.append(item["id"])
            for glyph in item["glyphs"]:
                a, b, c, d = glyph["bounds"]
                outline = max(.5, item["font"]*.055)
                if any(cv2.pointPolygonTest(polygon, (float(x), float(y)), False) < 0
                       for x in np.linspace(a-outline, c+outline, 3)
                       for y in np.linspace(b-outline, d+outline, 3)):
                    crossings.append(dict(region=item["id"], text=glyph["text"], bounds=glyph["bounds"]))
        original = cv2.imread(str(image), cv2.IMREAD_COLOR)
        outside_alpha_changes = 0
        for region in regions.values():
            patch = cv2.imdecode(np.frombuffer(base64.b64decode(region["background_png"]), np.uint8), cv2.IMREAD_UNCHANGED)
            a, b, c, d = region["background_box"]
            clean = patch[:, :, 3] == 0
            # Grayscale transport is expected only for neutral source pages.
            delta = np.max(np.abs(patch[:, :, :3].astype(int)-original[b:d, a:c].astype(int)), axis=2)
            outside_alpha_changes += int(np.count_nonzero(clean & (delta > 8)))
        shown = audit["rendered_ids"]
        normalize = lambda value: unicodedata.normalize("NFKC", value).strip()
        missing, preserved = [], []
        for block in page["blocks"]:
            if block["id"] in shown:
                continue
            entry = dict(id=block["id"], source=block["source"], translation=block["translation"], box=block["box"])
            if block["id"] in payload.get("preserved", {}):
                entry["reason"] = payload["preserved"][block["id"]]
                preserved.append(entry)
            elif not block["translation"].strip() or block.get("error") or normalize(block["source"]) == normalize(block["translation"]):
                preserved.append(entry)
            else:
                missing.append(entry)
        result = dict(page=index, image=image.name, blocks=len(page["blocks"]), displayed=len(shown),
                      missing=missing, preserved=preserved, glyph_crossings=crossings,
                      text_errors=text_errors, outside_alpha_changes=outside_alpha_changes,
                      originals_unchanged=sha(image)==image_hash and sha(cache)==cache_hash,
                      image_hash=image_hash, cache_hash=cache_hash,
                      seconds=round(time.monotonic()-start, 2))
        results.append(result)
        (args.output/"summary.json").write_text(json.dumps(results, ensure_ascii=False, indent=2))
        print(json.dumps({k:v for k,v in result.items() if k not in {"missing", "preserved", "glyph_crossings", "image_hash", "cache_hash"}}), flush=True)
    (args.output/"summary.json").write_text(json.dumps(results, ensure_ascii=False, indent=2))
    if any(r.get("error") or r.get("glyph_crossings") or r.get("text_errors") or r.get("outside_alpha_changes") or not r.get("originals_unchanged", False) for r in results):
        raise SystemExit("Review found verification failures; inspect summary.json")


if __name__ == "__main__":
    main()

"""Render cached manga pages with the production Qt canvas for visual review."""
import argparse
import json
import os
from pathlib import Path
import sys

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from PySide6.QtWidgets import QApplication
from PySide6.QtGui import QPixmap
from backend.worker import cache_path
from windows_app import ImageCanvas


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("images", nargs="+")
    parser.add_argument("--output", type=Path, default=ROOT / ".runtime" / "overlay-review")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    app = QApplication([])
    reports = []
    for filename in args.images:
        image = Path(filename)
        cache = cache_path(image)
        if not cache.exists():
            reports.append({"image": image.name, "error": "No OCR cache"})
            continue
        page = json.loads(cache.read_text(encoding="utf-8"))
        canvas = ImageCanvas()
        pixmap = QPixmap(str(image))
        canvas.set_image(pixmap)
        canvas.resize(pixmap.width() + 24, pixmap.height() + 24)
        canvas.set_page(page)
        canvas.set_mode(2)
        canvas.show()
        app.processEvents()
        output = args.output / (image.stem + "-chinese.png")
        canvas.grab().copy(canvas._image_rect().toRect()).save(str(output))
        reports.append({"image": image.name, "output": str(output),
                        "regions": [{"blocks": r["block_ids"], "overlay": r["overlay"],
                                     "text_box": r["text_box"],
                                     "font_px": canvas._overlay_layouts.get(r["id"], {}).get("font_px")}
                                    for r in page["overlay_regions"]],
                        "preserved": [b["id"] for b in page["blocks"]
                                      if b.get("overlay") in ("uncertain", "skip")]})
        canvas.close()
    (args.output / "report.json").write_text(json.dumps(reports, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(reports, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
